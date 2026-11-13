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
    sync::{Semaphore, mpsc},
    task::JoinHandle,
    time::timeout,
};
use tokio_util::codec::{FramedRead, FramedWrite};

/// Responses that may be queued before the reader stops taking requests.
const OUTBOUND_MESSAGES: usize = 4;

/// How long the reader waits for outbound space before closing the connection.
const OUTBOUND_WAIT: Duration = Duration::from_secs(5);

/// How long shutdown waits for queued output to flush.
const CLOSE_WAIT: Duration = Duration::from_secs(2);

/// The largest single inbound line, matching the engine frame limit.
const MAX_LINE_BYTES: usize = 16 * 1024 * 1024;

/// Standard-IO transport with a bounded outbound queue.
pub struct BoundedStdio {
    reader: FramedRead<StdioReader, JsonRpcMessageCodec<RxJsonRpcMessage<RoleServer>>>,
    sender: Option<mpsc::Sender<TxJsonRpcMessage<RoleServer>>>,
    slots: Arc<Semaphore>,
    writer: Option<JoinHandle<()>>,
    supported: Vec<ProtocolVersion>,
}

impl BoundedStdio {
    /// Build the transport over this process's standard streams.
    pub fn new(supported: Vec<ProtocolVersion>) -> Self {
        let (input, output): (StdioReader, StdioWriter) = stdio_pipes();
        let reader = FramedRead::new(
            input,
            JsonRpcMessageCodec::<RxJsonRpcMessage<RoleServer>>::new_with_max_length(
                MAX_LINE_BYTES,
            ),
        );
        let (sender, mut queued) = mpsc::channel::<TxJsonRpcMessage<RoleServer>>(OUTBOUND_MESSAGES);
        let slots = Arc::new(Semaphore::new(OUTBOUND_MESSAGES));
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
        match timeout(OUTBOUND_WAIT, Arc::clone(&self.slots).acquire()).await {
            Ok(Ok(permit)) => drop(permit),
            Ok(Err(_)) => return None,
            Err(_) => {
                eprintln!(
                    "au-mcp-rs: client stopped reading stdout for {}s; closing the connection",
                    OUTBOUND_WAIT.as_secs()
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
                    eprintln!("au-mcp-rs: rejecting unreadable input frame: {error}");
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
            && timeout(CLOSE_WAIT, writer).await.is_err()
        {
            eprintln!(
                "au-mcp-rs: queued output did not flush within {}s; abandoning it",
                CLOSE_WAIT.as_secs()
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
