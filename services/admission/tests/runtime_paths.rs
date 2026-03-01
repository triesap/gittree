use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

fn reserve_local_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind local port");
    let port = listener.local_addr().expect("listener addr").port();
    drop(listener);
    port
}

fn runtime_storage_url() -> String {
    std::env::var("GITTREE_STORAGE_TEST_DATABASE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "postgres://user:pass@127.0.0.1:5432/gittree".to_string())
}

fn spawn_admission_server(port: u16) -> (Child, String) {
    let bind = format!("127.0.0.1:{port}");

    let mut command = Command::new(env!("CARGO_BIN_EXE_gittree-admission"));
    command
        .env("GITTREE_ADMISSION_BIND", &bind)
        .env("GITTREE_STORAGE_READ_URL", runtime_storage_url())
        .env("GITTREE_LOG_STDOUT", "false")
        .env("GITTREE_METRICS_ENABLED", "false")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    let child = command.spawn().expect("spawn admission server");
    (child, bind)
}

fn http_request(bind: &str, request: &str) -> String {
    let mut stream = TcpStream::connect(bind).expect("connect server");
    stream
        .set_read_timeout(Some(Duration::from_millis(500)))
        .expect("set read timeout");
    stream
        .set_write_timeout(Some(Duration::from_millis(500)))
        .expect("set write timeout");
    stream.write_all(request.as_bytes()).expect("write request");
    let mut response = String::new();
    stream.read_to_string(&mut response).expect("read response");
    response
}

fn response_status_code(response: &str) -> u16 {
    response
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .unwrap_or(0)
}

fn wait_for_health(bind: &str) {
    for _ in 0..60 {
        if let Ok(mut stream) = TcpStream::connect(bind) {
            let _ = stream.set_read_timeout(Some(Duration::from_millis(250)));
            let _ = stream.set_write_timeout(Some(Duration::from_millis(250)));
            if stream
                .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                .is_ok()
            {
                let mut response = String::new();
                if stream.read_to_string(&mut response).is_ok()
                    && response.starts_with("HTTP/1.1 200")
                {
                    return;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("admission server never became ready");
}

fn stop_server(child: &mut Child) {
    let _ = Command::new("kill")
        .arg("-TERM")
        .arg(child.id().to_string())
        .status();

    for _ in 0..20 {
        if child.try_wait().ok().flatten().is_some() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn admission_binary_runtime_routes_cover_non_test_monomorphizations() {
    let port = reserve_local_port();
    let (mut child, bind) = spawn_admission_server(port);

    wait_for_health(&bind);

    let missing = http_request(
        &bind,
        "GET /missing HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    assert_eq!(response_status_code(&missing), 404, "{missing}");

    let invalid_decide = http_request(
        &bind,
        "POST /decide HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: 2\r\n\r\n{}",
    );
    let status = response_status_code(&invalid_decide);
    assert!(
        (400..500).contains(&status),
        "expected 4xx for invalid decide payload, got {status}: {invalid_decide}"
    );

    stop_server(&mut child);
}
