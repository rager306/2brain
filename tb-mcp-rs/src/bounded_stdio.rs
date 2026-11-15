//! Bounded standard-IO transport for the MCP server.
//!
//! The SDK transport writes responses from spawned tasks and keeps reading
//! standard input while an output write is pending. A client that stops
//! draining standard output therefore grows memory without bound and can keep
//! the process alive after input closes.
//!
//! This transport bounds that: at most [`OUTBOUND_MESSAGES`] responses may be
//! queued, and the reader refuses to take another request while the queue is
//! full, so the client's own pipe buffer becomes the backpressure signal. If
//! the client does not drain output for [`OUTBOUND_WAIT`], the connection is
//! closed instead of buffering further. Shutdown waits at most [`CLOSE_WAIT`]
//! for queued output.

use std::{future::Future, io, sync::Arc, time::Duration};

use futures::{SinkExt, StreamExt};
use rmcp::{
    RoleServer,
    model::{ClientRequest, ErrorData, GetMeta, PingRequestMethod, ProtocolVersion},
    service::{RxJsonRpcMessage, TxJsonRpcMessage},
    transport::{
        Transport,
        async_rw::JsonRpcMessageCodec,
        io::{StdioReader, StdioWriter, stdio_pipes},
    },
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::{Semaphore, mpsc},
    task::JoinHandle,
    time::timeout,
};
use tokio_util::codec::{FramedRead, FramedWrite};

/// The transport's limits. A parameter rather than four constants so a test can drive
/// the backpressure with a small queue instead of waiting out the production timeouts.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Bounds {
    /// Responses that may be queued before the reader stops taking requests.
    pub outbound_messages: usize,
    /// How long the reader waits for outbound space before closing the connection.
    pub outbound_wait: Duration,
    /// How long shutdown waits for queued output to flush.
    pub close_wait: Duration,
    /// The largest single inbound line, matching the engine frame limit.
    pub max_line_bytes: usize,
}

impl Default for Bounds {
    fn default() -> Self {
        Self {
            outbound_messages: 4,
            outbound_wait: Duration::from_secs(5),
            close_wait: Duration::from_secs(2),
            max_line_bytes: 16 * 1024 * 1024,
        }
    }
}

/// Standard-IO transport with a bounded outbound queue.
/// A stream pair is chosen at construction (`stdio_pipes()` in production, a duplex pair
/// in tests), so the transport holds it behind one box rather than a type parameter.
type BoxedReader = Box<dyn AsyncRead + Send + Unpin>;
type BoxedWriter = Box<dyn AsyncWrite + Send + Unpin>;

pub struct BoundedStdio {
    reader: FramedRead<BoxedReader, JsonRpcMessageCodec<RxJsonRpcMessage<RoleServer>>>,
    sender: Option<mpsc::Sender<TxJsonRpcMessage<RoleServer>>>,
    slots: Arc<Semaphore>,
    writer: Option<JoinHandle<()>>,
    supported: Vec<ProtocolVersion>,
    bounds: Bounds,
}

impl BoundedStdio {
    /// Build the transport over this process's standard streams with the default limits.
    pub fn new(supported: Vec<ProtocolVersion>) -> Self {
        let (input, output): (StdioReader, StdioWriter) = stdio_pipes();
        Self::over(input, output, supported, Bounds::default())
    }

    /// Build the transport over any stream pair, so the bounds are testable without a
    /// process's standard streams.
    pub(crate) fn over<R, W>(
        input: R,
        output: W,
        supported: Vec<ProtocolVersion>,
        bounds: Bounds,
    ) -> Self
    where
        R: AsyncRead + Send + Unpin + 'static,
        W: AsyncWrite + Send + Unpin + 'static,
    {
        let input: BoxedReader = Box::new(input);
        let output: BoxedWriter = Box::new(output);
        let reader = FramedRead::new(
            input,
            JsonRpcMessageCodec::<RxJsonRpcMessage<RoleServer>>::new_with_max_length(
                bounds.max_line_bytes,
            ),
        );
        let (sender, mut queued) =
            mpsc::channel::<TxJsonRpcMessage<RoleServer>>(bounds.outbound_messages);
        let slots = Arc::new(Semaphore::new(bounds.outbound_messages));
        let writer_slots = Arc::clone(&slots);
        let writer = tokio::spawn(async move {
            let mut sink = FramedWrite::new(
                output,
                JsonRpcMessageCodec::<TxJsonRpcMessage<RoleServer>>::default(),
            );
            while let Some(message) = queued.recv().await {
                if sink.send(message).await.is_err() {
                    break;
                }
                writer_slots.add_permits(1);
            }
            let _ = sink.flush().await;
        });
        Self {
            reader,
            sender: Some(sender),
            slots,
            writer: Some(writer),
            supported,
            bounds,
        }
    }

    fn closed() -> io::Error {
        io::Error::new(io::ErrorKind::NotConnected, "transport is closed")
    }
}

impl Transport<RoleServer> for BoundedStdio {
    type Error = io::Error;

    fn send(
        &mut self,
        item: TxJsonRpcMessage<RoleServer>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        let sender = self.sender.clone();
        let slots = Arc::clone(&self.slots);
        async move {
            let Some(sender) = sender else {
                return Err(Self::closed());
            };
            let Ok(permit) = slots.acquire_owned().await else {
                return Err(Self::closed());
            };
            // The permit is returned by the writer once the message is written,
            // so a queued response keeps occupying its slot.
            permit.forget();
            sender.send(item).await.map_err(|_| Self::closed())
        }
    }

    async fn receive(&mut self) -> Option<RxJsonRpcMessage<RoleServer>> {
        match timeout(self.bounds.outbound_wait, Arc::clone(&self.slots).acquire()).await {
            Ok(Ok(permit)) => drop(permit),
            Ok(Err(_)) => return None,
            Err(_) => {
                eprintln!(
                    "tb-mcp-rs: client stopped reading stdout for {}s; closing the connection",
                    self.bounds.outbound_wait.as_secs()
                );
                // Drop queued output instead of waiting for a client that is not
                // reading, so shutdown does not stall on it.
                self.slots.close();
                return None;
            }
        }
        loop {
            match self.reader.next().await {
                Some(Ok(message)) => {
                    // A `ping` before the lifecycle is chosen is answered by the
                    // SDK without inspecting its `_meta`, but 2026-07-28 requires
                    // the version to be checked first and removes `ping`
                    // altogether. Refuse such a request here, where the raw
                    // message is still visible.
                    if let Some(rejection) = version_checked_ping(&message, &self.supported) {
                        if let Some(sender) = self.sender.as_ref()
                            && sender.send(rejection).await.is_err()
                        {
                            return None;
                        }
                        continue;
                    }
                    return Some(message);
                }
                Some(Err(error)) => {
                    eprintln!("tb-mcp-rs: rejecting unreadable input frame: {error}");
                    if let Some(sender) = self.sender.as_ref() {
                        let rejection = TxJsonRpcMessage::<RoleServer>::error(
                            ErrorData::invalid_request("Invalid request", None),
                            None,
                        );
                        if sender.send(rejection).await.is_err() {
                            return None;
                        }
                    }
                }
                None => return None,
            }
        }
    }

    async fn close(&mut self) -> Result<(), Self::Error> {
        // Fail any send still waiting for a slot, so the SDK's shutdown drain
        // does not wait on output that cannot be written.
        self.slots.close();
        self.sender = None;
        if let Some(writer) = self.writer.take()
            && timeout(self.bounds.close_wait, writer).await.is_err()
        {
            eprintln!(
                "tb-mcp-rs: queued output did not flush within {}s; abandoning it",
                self.bounds.close_wait.as_secs()
            );
        }
        Ok(())
    }
}

/// Refuse a pre-lifecycle `ping` that declares a protocol version.
///
/// An unsupported version is `-32022`; a supported 2026-07-28-or-later version
/// no longer has a `ping` method and gets `-32601`. A `ping` without a declared
/// version keeps the legacy behaviour the specification allows before
/// initialization.
fn version_checked_ping(
    message: &RxJsonRpcMessage<RoleServer>,
    supported: &[ProtocolVersion],
) -> Option<TxJsonRpcMessage<RoleServer>> {
    let RxJsonRpcMessage::<RoleServer>::Request(request) = message else {
        return None;
    };
    if !matches!(request.request, ClientRequest::PingRequest(_)) {
        return None;
    }
    let version = request.request.get_meta().protocol_version()?;
    let id: Option<rmcp::model::RequestId> = Some(request.id.clone());
    if !supported.contains(&version) {
        return Some(TxJsonRpcMessage::<RoleServer>::error(
            ErrorData::unsupported_protocol_version(version, supported),
            id,
        ));
    }
    if version.as_str() >= ProtocolVersion::V_2026_07_28.as_str() {
        return Some(TxJsonRpcMessage::<RoleServer>::error(
            ErrorData::method_not_found::<PingRequestMethod>(),
            id,
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, duplex};

    /// A response small enough to fit the codec but large enough that a 16-byte socket
    /// buffer cannot absorb it, so the writer really blocks.
    fn a_response() -> TxJsonRpcMessage<RoleServer> {
        TxJsonRpcMessage::<RoleServer>::error(
            ErrorData::invalid_request("a message long enough to fill the pipe", None),
            None,
        )
    }

    #[tokio::test]
    async fn the_queue_blocks_a_send_until_the_client_reads() {
        let (mut client, server) = duplex(16);
        let (input, output) = tokio::io::split(server);
        let bounds = Bounds {
            outbound_messages: 2,
            ..Bounds::default()
        };
        let mut transport = BoundedStdio::over(input, output, Vec::new(), bounds);

        // Two sends fill every permit. The writer cannot flush into a 16-byte pipe
        // nobody is reading, so no permit comes back.
        for _ in 0..2 {
            tokio::time::timeout(Duration::from_secs(1), transport.send(a_response()))
                .await
                .expect("the first sends fit the queue")
                .expect("send");
        }
        let blocked =
            tokio::time::timeout(Duration::from_millis(50), transport.send(a_response())).await;
        assert!(
            blocked.is_err(),
            "a third send must wait: the queue is full and no permit was returned"
        );

        // Draining the client lets the writer finish, and the waiting send proceeds.
        let drain = tokio::spawn(async move {
            let mut sink = Vec::new();
            let _ =
                tokio::time::timeout(Duration::from_secs(2), client.read_to_end(&mut sink)).await;
        });
        let sent = tokio::time::timeout(Duration::from_secs(2), transport.send(a_response())).await;
        assert!(
            sent.is_ok(),
            "draining the pipe must release the waiting send"
        );
        assert!(sent.unwrap().is_ok());
        transport.close().await.expect("close");
        drain.abort();
    }

    #[tokio::test]
    async fn a_closed_transport_refuses_a_send() {
        let (client, server) = duplex(64);
        let (input, output) = tokio::io::split(server);
        let mut transport = BoundedStdio::over(input, output, Vec::new(), Bounds::default());
        transport.close().await.expect("close");
        let error = transport.send(a_response()).await;
        assert!(
            error.is_err(),
            "a closed transport must not accept a response"
        );
        drop(client);
    }
}
