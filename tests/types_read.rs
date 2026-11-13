use std::{
    io::{BufRead, BufReader, Read, Write},
    os::unix::net::UnixListener,
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::Duration,
};

use serde_json::{Value, json};

struct Client {
    child: Child,
    input: ChildStdin,
    responses: Receiver<Value>,
}

impl Client {
    fn start(socket: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_au-mcp-rs"))
            .arg(socket)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, responses) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                if tx
                    .send(serde_json::from_str(&line.unwrap()).unwrap())
                    .is_err()
                {
                    break;
                }
            }
        });
        let mut client = Self {
            child,
            input,
            responses,
        };
        let response = client.request(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"types-test","version":"1"}}}));
        assert!(response.get("result").is_some(), "{response}");
        writeln!(
            client.input,
            "{}",
            json!({"jsonrpc":"2.0","method":"notifications/initialized"})
        )
        .unwrap();
        client.input.flush().unwrap();
        client
    }

    fn request(&mut self, request: Value) -> Value {
        writeln!(self.input, "{request}").unwrap();
        self.input.flush().unwrap();
        self.responses
            .recv_timeout(Duration::from_secs(5))
            .expect("MCP response timeout")
    }

    fn call(&mut self, id: u32, arguments: Value) -> Value {
        let response = self.request(json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"au_types","arguments":arguments}}));
        assert_eq!(response["id"], id, "{response}");
        response
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn assert_error(response: &Value) {
    assert!(
        response.get("error").is_some() || response["result"]["isError"] == true,
        "{response}"
    );
}

fn result_payload(response: &Value) -> Value {
    assert!(response.get("error").is_none(), "{response}");
    assert_ne!(response["result"]["isError"], true, "{response}");
    serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
}

fn read_request(stream: &mut impl Read) -> Value {
    let mut len = [0; 4];
    stream.read_exact(&mut len).unwrap();
    let mut body = vec![0; u32::from_be_bytes(len) as usize];
    stream.read_exact(&mut body).unwrap();
    serde_json::from_slice(&body).unwrap()
}

fn fake_daemon(listener: UnixListener, cases: Vec<(Value, Value)>) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        for (request, reply) in cases {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            assert_eq!(read_request(&mut stream), request);
            let body = serde_json::to_vec(&reply).unwrap();
            stream
                .write_all(&(body.len() as u32).to_be_bytes())
                .unwrap();
            stream.write_all(&body).unwrap();
        }
    })
}

fn response(ready: bool, result: Value) -> Value {
    json!({"type":"response","schema_version":29,"id":1,"ready":ready,"version":1,"result":result})
}

fn default_request() -> Value {
    json!({"read":"types","id":1,"summary":true,"limit":50,"offset":0})
}

#[test]
fn invalid_arguments_never_contact_engine() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut client = Client::start(&socket);
    for (index, args) in [
        json!({"repo":3}),
        json!({"summary":null}),
        json!({"summary":"true"}),
        json!({"limit":0}),
        json!({"limit":501}),
        json!({"limit":-1}),
        json!({"limit":1.5}),
        json!({"limit":null}),
        json!({"offset":-1}),
        json!({"offset":1.5}),
        json!({"offset":null}),
        json!({"unknown":true}),
        json!({"read":"overview"}),
        json!({"id":9}),
        json!({"write":"delete"}),
        json!([]),
        json!("types"),
        json!(123),
    ]
    .into_iter()
    .enumerate()
    {
        assert_error(&client.call(index as u32 + 2, args));
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
}

#[test]
fn defaults_pagination_repo_and_results_are_preserved() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let cases = [
        (json!({}), default_request(), json!({"types":[]})),
        (json!(null), default_request(), json!({"types":null})),
        (
            json!({"repo":null}),
            default_request(),
            json!({"types":null}),
        ),
        (
            json!({"repo":""}),
            json!({"read":"types","id":1,"repo":"","summary":true,"limit":50,"offset":0}),
            json!({"types":null}),
        ),
        (
            json!({"repo":"work","summary":false,"limit":1,"offset":7}),
            json!({"read":"types","id":1,"repo":"work","summary":false,"limit":1,"offset":7}),
            json!({"types":[{"name":"task","repo":"work","extra":true}],"extra":{"future":true}}),
        ),
        (
            json!({"limit":500,"offset":0}),
            json!({"read":"types","id":1,"summary":true,"limit":500,"offset":0}),
            json!({"types":[]}),
        ),
    ];
    let daemon = fake_daemon(
        listener,
        cases
            .iter()
            .map(|(_, request, payload)| (request.clone(), response(true, payload.clone())))
            .chain(std::iter::once((
                default_request(),
                response(true, json!({"types":[]})),
            )))
            .collect(),
    );
    let mut client = Client::start(&socket);
    for (index, (args, _, payload)) in cases.into_iter().enumerate() {
        assert_eq!(
            result_payload(&client.call(index as u32 + 2, args)),
            payload
        );
    }
    let omitted = client.request(
        json!({"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"au_types"}}),
    );
    assert_eq!(omitted["id"], 8);
    assert_eq!(result_payload(&omitted), json!({"types":[]}));
    daemon.join().unwrap();
}

#[test]
fn malformed_results_engine_error_and_not_ready_recover() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("engine.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let replies = [
        response(true, json!({})),
        response(true, json!({"types":{}})),
        response(true, json!({"types":"task"})),
        response(true, json!({"types":1})),
        json!({"type":"error","schema_version":29,"id":1,"for":"read","error":"engine failure"}),
        response(false, json!({"types":[]})),
        response(true, json!({"types":[{"name":"task"}]})),
    ];
    let daemon = fake_daemon(
        listener,
        replies
            .into_iter()
            .map(|reply| (default_request(), reply))
            .collect(),
    );
    let mut client = Client::start(&socket);
    for id in 2..8 {
        assert_error(&client.call(id, json!({})));
    }
    assert_eq!(
        result_payload(&client.call(8, json!({}))),
        json!({"types":[{"name":"task"}]})
    );
    daemon.join().unwrap();
}
