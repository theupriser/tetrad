use std::io::{self, Cursor, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use tetrad_remote::{Call, MAX_BODY, Reply, Request, Server, serve_one};

/// A connection: the request bytes in, the response bytes out.
struct Pipe {
    input: Cursor<Vec<u8>>,
    output: Vec<u8>,
}

impl Read for Pipe {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.input.read(buf)
    }
}

impl Write for Pipe {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.output.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Serves `request`; the device answers `reply`. Returns the status line, the whole response
/// and the request the device saw.
fn exchange(request: &[u8], reply: Reply) -> (String, String, Option<Request>) {
    let mut pipe = Pipe {
        input: Cursor::new(request.to_vec()),
        output: Vec::new(),
    };
    let mut seen = None;
    serve_one(&mut pipe, |request| {
        seen = Some(request);
        reply
    })
    .unwrap();
    let response = String::from_utf8(pipe.output).unwrap();
    let status = response.lines().next().unwrap_or_default().to_owned();
    (status, response, seen)
}

fn upload(head: &str, body: &[u8]) -> Vec<u8> {
    let mut request = head.as_bytes().to_vec();
    request.extend_from_slice(body);
    request
}

/// The page and the two device routes: the PIN, the decoded name and the body reach the
/// device; its reply becomes the status code and JSON body.
#[test]
fn routes_reach_the_device_with_pin_name_and_body() {
    let (status, response, seen) =
        exchange(b"GET / HTTP/1.1\r\nHost: 192.168.1.20\r\n\r\n", Reply::Off);
    assert_eq!(status, "HTTP/1.1 200 OK");
    assert!(response.contains("Content-Security-Policy: default-src 'none'"));
    assert!(response.contains("<title>Tetrad Remote</title>"));
    assert_eq!(seen, None);

    let status_request =
        b"GET /api/status HTTP/1.1\r\nHost: 192.168.1.20\r\nX-Tetrad-Pin: 4821\r\n\r\n";
    let folder = Reply::Status {
        folder: "IRs".into(),
    };
    let (status, response, seen) = exchange(status_request, folder);
    assert_eq!(status, "HTTP/1.1 200 OK");
    assert!(response.ends_with(r#"{"folder":"IRs"}"#));
    assert_eq!(seen, Some(Request::Status { pin: "4821".into() }));

    let wav = [7_u8; 100];
    let head = "POST /api/irs?x=1&name=V30%20SM57+%C3%A9 HTTP/1.1\r\nHost: 192.168.1.20\r\n\
                x-tetrad-pin: 4821\r\nContent-Length: 100\r\n\r\n";
    let added = Reply::Added {
        name: "V30 SM57 é".into(),
        folder: "IRs".into(),
    };
    let (status, _, seen) = exchange(&upload(head, &wav), added);
    assert_eq!(status, "HTTP/1.1 200 OK");
    assert_eq!(
        seen,
        Some(Request::Upload {
            pin: "4821".into(),
            name: "V30 SM57 é".into(),
            wav: wav.to_vec(),
        })
    );

    let (status, response, _) = exchange(status_request, Reply::WrongPin);
    assert_eq!(status, "HTTP/1.1 401 Unauthorized");
    assert!(response.ends_with(r#"{"error":"Wrong PIN"}"#));
    let refused = Reply::Refused("the library is closed".into());
    assert_eq!(
        exchange(status_request, refused).0,
        "HTTP/1.1 422 Unprocessable Content"
    );
}

/// Requests the server refuses itself never reach the device.
#[test]
fn bad_requests_are_refused_without_a_device_call() {
    let big = "a".repeat(8 * 1024);
    let cases: [(Vec<u8>, &str); 10] = [
        (b"GET /api/status HTTP/1.1\r\nHost: 192.168.1.20\r\n\r\n".to_vec(), "401"),
        (
            b"GET /api/status HTTP/1.1\r\nHost: 192.168.1.20\r\nX-Tetrad-Pin:\r\n\r\n".to_vec(),
            "401",
        ),
        (
            b"POST /api/irs?name=a HTTP/1.1\r\nHost: 192.168.1.20\r\nX-Tetrad-Pin: 1\r\n\r\n".to_vec(),
            "411",
        ),
        (
            format!(
                "POST /api/irs?name=a HTTP/1.1\r\nHost: 192.168.1.20\r\nX-Tetrad-Pin: 1\r\nContent-Length: {}\r\n\r\n",
                MAX_BODY + 1
            )
            .into_bytes(),
            "413",
        ),
        (
            upload(
                "POST /api/irs?name=%20 HTTP/1.1\r\nHost: 192.168.1.20\r\nX-Tetrad-Pin: 1\r\nContent-Length: 1\r\n\r\n",
                b"x",
            ),
            "400",
        ),
        (b"DELETE /api/irs HTTP/1.1\r\nHost: 192.168.1.20\r\n\r\n".to_vec(), "405"),
        (b"GET /secret HTTP/1.1\r\nHost: 192.168.1.20\r\n\r\n".to_vec(), "404"),
        // DNS rebinding: a name, or no Host at all.
        (b"GET / HTTP/1.1\r\nHost: qc.example:80\r\n\r\n".to_vec(), "421"),
        (b"GET / HTTP/1.0\r\n\r\n".to_vec(), "421"),
        // Over the 8 KiB limit, though one read completes the header.
        (
            format!("GET / HTTP/1.1\r\nHost: 192.168.1.20\r\nX-Big: {big}\r\n\r\n").into_bytes(),
            "431",
        ),
    ];
    for (request, code) in cases {
        let (status, _, seen) = exchange(&request, Reply::Off);
        assert!(status.contains(code), "{status} for {request:?}");
        assert_eq!(seen, None, "{request:?}");
    }
}

/// The real thread over loopback: the call reaches `drain`, and `reply` answers the open
/// connection.
#[test]
fn server_round_trip_over_loopback() {
    let mut server = Server::start(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).unwrap();
    let address = server.local_addr();
    let client = std::thread::spawn(move || {
        let mut stream = TcpStream::connect(address).unwrap();
        stream
            .write_all(
                b"GET /api/status HTTP/1.1\r\nHost: 192.168.1.20\r\nX-Tetrad-Pin: 4821\r\n\r\n",
            )
            .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut calls = Vec::new();
    while calls.is_empty() && Instant::now() < deadline {
        server.drain(&mut calls);
        std::thread::sleep(Duration::from_millis(5));
    }
    let [Call { id, request }] = calls.as_slice() else {
        panic!("one call, got {calls:?}");
    };
    assert_eq!(*request, Request::Status { pin: "4821".into() });
    server.reply(
        *id,
        Reply::Status {
            folder: "IRs".into(),
        },
    );
    let response = client.join().unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    assert!(response.ends_with(r#"{"folder":"IRs"}"#));
}

/// Stop closes the port even while a client holds the connection open: Remote turned off and
/// on again binds the same port.
#[test]
fn stop_frees_the_port_while_a_client_waits() {
    let server = Server::start(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).unwrap();
    let address = server.local_addr();
    let mut client = TcpStream::connect(address).unwrap();
    client.write_all(b"GET / HTTP/1.1\r\n").unwrap();
    // Time for the thread to take the connection and wait for the rest.
    std::thread::sleep(Duration::from_millis(300));
    let stopping = Instant::now();
    drop(server);
    // Bounded by the accept poll, not the 10 s connection deadline.
    assert!(
        stopping.elapsed() < Duration::from_secs(1),
        "{:?}",
        stopping.elapsed()
    );
    Server::start(address).expect("the port is free when drop returns");
    // The waiting client was closed, not left until its deadline.
    client
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    assert_eq!(client.read(&mut [0; 16]).unwrap(), 0);
}

/// A dev build takes an update in three steps; each reaches the device with the PIN, and the
/// reply of the device becomes the answer. Without the `dev` feature none of them exists.
#[cfg(feature = "dev")]
#[test]
fn the_update_routes_reach_the_device() {
    let head = |method: &str, path: &str, body: &str| {
        format!(
            "{method} {path} HTTP/1.1\r\nHost: 192.168.1.20\r\nX-Tetrad-Pin: 4821\r\n\
             Content-Length: {}\r\n\r\n{body}",
            body.len()
        )
    };
    let pin = || "4821".to_owned();

    let reply = Reply::UpdateStatus {
        version: "0.1.0".into(),
    };
    let (status, response, seen) = exchange(head("GET", "/api/update", "").as_bytes(), reply);
    assert_eq!(status, "HTTP/1.1 200 OK");
    assert!(response.ends_with(r#"{"version":"0.1.0"}"#));
    assert_eq!(seen, Some(Request::UpdateStatus { pin: pin() }));

    let body = r#"{"files":{"bin/tetrad":7,"launcher/tetrad-update.sh":12}}"#;
    let begun = Reply::UpdateBegun {
        version: "1.0.0+http".into(),
        files: vec![("bin/tetrad".into(), 7)],
    };
    let (status, response, seen) =
        exchange(head("POST", "/api/update/begin", body).as_bytes(), begun);
    assert_eq!(status, "HTTP/1.1 200 OK");
    assert!(
        response.ends_with(r#"{"files":[{"path":"bin/tetrad","size":7}],"version":"1.0.0+http"}"#)
    );
    assert_eq!(
        seen,
        Some(Request::UpdateBegin {
            pin: pin(),
            files: vec![
                ("bin/tetrad".into(), 7),
                ("launcher/tetrad-update.sh".into(), 12)
            ],
        })
    );

    let path = "/api/update/chunk?path=launcher%2Ftetrad-update.sh&offset=4096";
    let received = Reply::UpdateReceived {
        path: "launcher/tetrad-update.sh".into(),
        received: 4100,
    };
    let (status, _, seen) = exchange(head("POST", path, "data").as_bytes(), received);
    assert_eq!(status, "HTTP/1.1 200 OK");
    assert_eq!(
        seen,
        Some(Request::UpdateChunk {
            pin: pin(),
            path: "launcher/tetrad-update.sh".into(),
            offset: 4096,
            bytes: b"data".to_vec(),
        })
    );

    let staged = Reply::UpdateStaged {
        version: "1.0.0".into(),
    };
    let (status, _, seen) = exchange(head("POST", "/api/update/ready", "").as_bytes(), staged);
    assert_eq!(status, "HTTP/1.1 200 OK");
    assert_eq!(seen, Some(Request::UpdateReady { pin: pin() }));

    // A refusal of the device is a 422 with its reason.
    let refused = Reply::Refused("no install-begin".into());
    let (status, response, _) = exchange(head("POST", "/api/update/ready", "").as_bytes(), refused);
    assert_eq!(status, "HTTP/1.1 422 Unprocessable Content");
    assert!(response.ends_with(r#"{"error":"no install-begin"}"#));
}

/// The update routes need the PIN, a file list as JSON and a numeric offset, and take no other method.
#[cfg(feature = "dev")]
#[test]
fn the_update_routes_check_their_input() {
    let case = |request: &str| exchange(request.as_bytes(), Reply::Off);
    let host = "Host: 192.168.1.20\r\n";
    let (status, _, seen) = case(&format!("GET /api/update HTTP/1.1\r\n{host}\r\n"));
    assert_eq!(status, "HTTP/1.1 401 Unauthorized");
    assert_eq!(seen, None);
    let post = |path: &str, body: &str| {
        format!(
            "POST {path} HTTP/1.1\r\n{host}X-Tetrad-Pin: 1\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
    };
    for (path, body) in [
        ("/api/update/begin", "not json"),
        ("/api/update/begin", r#"{"manifest":"{}"}"#),
        ("/api/update/begin", r#"{"files":{"bin/tetrad":"big"}}"#),
        ("/api/update/begin", r#"{"files":["bin/tetrad"]}"#),
        ("/api/update/chunk?path=bin%2Ftetrad&offset=x", ""),
        ("/api/update/chunk?offset=0", ""),
    ] {
        let (status, _, seen) = case(&post(path, body));
        assert_eq!(status, "HTTP/1.1 400 Bad Request", "{path} {body}");
        assert_eq!(seen, None);
    }
    let (status, _, _) = case(&format!(
        "GET /api/update/ready HTTP/1.1\r\n{host}X-Tetrad-Pin: 1\r\n\r\n"
    ));
    assert_eq!(status, "HTTP/1.1 405 Method Not Allowed");
}

/// The page of a dev build has the update part; a release build has neither it nor the routes.
#[test]
fn only_a_dev_page_offers_the_update() {
    let (_, page, _) = exchange(b"GET / HTTP/1.1\r\nHost: 192.168.1.20\r\n\r\n", Reply::Off);
    assert_eq!(page.contains("Update Tetrad"), cfg!(feature = "dev"));
    assert!(!page.contains("<!--update-->"));
    if !cfg!(feature = "dev") {
        let request = b"GET /api/update HTTP/1.1\r\nHost: 192.168.1.20\r\nX-Tetrad-Pin: 1\r\n\r\n";
        let (status, _, seen) = exchange(request, Reply::Off);
        assert_eq!(status, "HTTP/1.1 404 Not Found");
        assert_eq!(seen, None);
    }
}

/// The log of the unit is plain text, one notice per line, behind the PIN (a dev build).
#[cfg(feature = "dev")]
#[test]
fn the_log_is_plain_text_behind_the_pin() {
    let request = b"GET /api/log HTTP/1.1\r\nHost: 192.168.1.20\r\nX-Tetrad-Pin: 4821\r\n\r\n";
    let text = "    12.500 Remote: update of bin/tetrad\n    13.000 Usb: hello\n";
    let reply = Reply::Log { text: text.into() };
    let (status, response, seen) = exchange(request, reply);
    assert_eq!(status, "HTTP/1.1 200 OK");
    assert!(response.contains("Content-Type: text/plain; charset=utf-8"));
    assert!(response.ends_with(text));
    assert_eq!(seen, Some(Request::Log { pin: "4821".into() }));

    let (status, _, seen) = exchange(
        b"GET /api/log HTTP/1.1\r\nHost: 192.168.1.20\r\n\r\n",
        Reply::Off,
    );
    assert_eq!(status, "HTTP/1.1 401 Unauthorized");
    assert_eq!(seen, None);
}
