//! A canned-response HTTP server, for the tests that have to drive a real client.
//!
//! The updater's resume protocol turns on what the server does with a `Range` header, and its
//! manifest fetch is a fail-closed ladder over response codes. Neither can be pinned by reading
//! source: what they answer to is a socket. Everything else in the tree that touches the network
//! is held by a source walk instead, because asserting a call did *not* happen needs no server.
//!
//! Hand-rolled against `std::net` rather than taken as a dependency. `.claude/rules/testing.md`
//! asks for an argument before a framework arrives, and the argument here comes out the other
//! way: enough HTTP/1.1 to serve a status, some headers and a body is this file, and a mock
//! framework's matching DSL is the half these suites would not use.

use std::io::{BufRead, BufReader, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;

/// What the client asked for. Bodies are not read — every request these suites make is a GET.
#[derive(Debug, Clone)]
pub struct TestRequest {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
}

impl TestRequest {
    /// Case-insensitive, as HTTP/1.1 field names are.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(field, _)| field.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// What to answer with. Built by the handler, which sees the request first.
#[derive(Debug, Clone)]
pub struct TestResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl TestResponse {
    #[must_use]
    pub fn ok(body: impl Into<Vec<u8>>) -> Self {
        Self {
            status: 200,
            headers: Vec::new(),
            body: body.into(),
        }
    }

    #[must_use]
    pub fn status(status: u16) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    #[must_use]
    pub fn body(mut self, body: impl Into<Vec<u8>>) -> Self {
        self.body = body.into();
        self
    }

    #[must_use]
    pub fn header(mut self, field: &str, value: impl Into<String>) -> Self {
        self.headers.push((field.to_owned(), value.into()));
        self
    }
}

/// A server on an ephemeral loopback port, serving whatever its handler returns until it is
/// dropped. Requests are recorded so a test can assert on what the client sent — or, for the
/// resume protocol's `Skip`, that it sent nothing at all.
pub struct TestServer {
    addr: SocketAddr,
    requests: Arc<Mutex<Vec<TestRequest>>>,
    running: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl TestServer {
    /// Binds port 0 and starts serving. The handler runs on the accept thread, one connection at
    /// a time, which is all these suites need and keeps ordering assertions meaningful.
    pub fn start<H>(handler: H) -> std::io::Result<Self>
    where
        H: Fn(&TestRequest) -> TestResponse + Send + 'static,
    {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let addr = listener.local_addr()?;
        let requests = Arc::new(Mutex::new(Vec::new()));
        let running = Arc::new(AtomicBool::new(true));

        let worker = {
            let requests = Arc::clone(&requests);
            let running = Arc::clone(&running);
            std::thread::Builder::new().name("test-http".into()).spawn(move || {
                for stream in listener.incoming() {
                    if !running.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(stream) = stream else { continue };
                    let _ = serve_one(stream, &handler, &requests);
                }
            })?
        };

        Ok(Self {
            addr,
            requests,
            running,
            worker: Some(worker),
        })
    }

    /// The origin to hand whatever is under test, with no trailing separator.
    #[must_use]
    pub fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    #[must_use]
    pub fn requests(&self) -> Vec<TestRequest> {
        self.requests.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        // A blocked `accept` can't be interrupted portably, so connect to it once and let the
        // loop notice the flag it now reads as false.
        let _ = TcpStream::connect(self.addr);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn serve_one<H>(
    mut stream: TcpStream,
    handler: &H,
    requests: &Mutex<Vec<TestRequest>>,
) -> std::io::Result<()>
where
    H: Fn(&TestRequest) -> TestResponse,
{
    let Some(request) = read_request(&stream)? else {
        return Ok(());
    };
    requests.lock().unwrap_or_else(PoisonError::into_inner).push(request.clone());

    let response = handler(&request);
    write_response(&mut stream, &response)?;
    let _ = stream.shutdown(Shutdown::Both);
    Ok(())
}

/// `None` for a connection that carried no request line, which is what the shutdown poke is.
fn read_request(stream: &TcpStream) -> std::io::Result<Option<TestRequest>> {
    let mut reader = BufReader::new(stream);
    let mut start = String::new();
    if reader.read_line(&mut start)? == 0 {
        return Ok(None);
    }
    let mut parts = start.split_whitespace();
    let (Some(method), Some(path)) = (parts.next(), parts.next()) else {
        return Ok(None);
    };
    let (method, path) = (method.to_owned(), path.to_owned());

    let mut headers = Vec::new();
    loop {
        let mut field = String::new();
        if reader.read_line(&mut field)? == 0 {
            break;
        }
        let field = field.trim_end();
        if field.is_empty() {
            break;
        }
        if let Some((name, value)) = field.split_once(':') {
            headers.push((name.trim().to_owned(), value.trim().to_owned()));
        }
    }

    Ok(Some(TestRequest {
        method,
        path,
        headers,
    }))
}

fn write_response(stream: &mut TcpStream, response: &TestResponse) -> std::io::Result<()> {
    let mut lines = vec![format!(
        "HTTP/1.1 {} {}",
        response.status,
        reason_phrase(response.status)
    )];
    lines.extend(response.headers.iter().map(|(field, value)| format!("{field}: {value}")));

    // A 304 carries no body by definition, and stating a length it will not send hangs a client
    // that believes it. Everything else declares one so the read ends without waiting on close.
    let bodyless = response.status == 304;
    if !bodyless {
        lines.push(format!("Content-Length: {}", response.body.len()));
    }
    lines.push("Connection: close".to_owned());

    stream.write_all(format!("{}\r\n\r\n", lines.join("\r\n")).as_bytes())?;
    if !bodyless {
        stream.write_all(&response.body)?;
    }
    stream.flush()
}

/// Cosmetic — no client branches on it — but a packet capture is unreadable without one.
fn reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        206 => "Partial Content",
        304 => "Not Modified",
        403 => "Forbidden",
        404 => "Not Found",
        416 => "Range Not Satisfiable",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Unspecified",
    }
}
