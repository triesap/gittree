use gittree_state::{StateConfig, serve};
use gittree_storage::StorageConfig;
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::time::Duration;

fn reserve_local_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind local port");
    let port = listener.local_addr().expect("local addr").port();
    drop(listener);
    port
}

fn request_status(addr: &str, path: &str) -> Option<u16> {
    let socket_addr: SocketAddr = addr.parse().ok()?;
    let mut stream = TcpStream::connect_timeout(&socket_addr, Duration::from_millis(200)).ok()?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
    let request = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).ok()?;
    let mut reader = BufReader::new(stream);
    let mut status_line = String::new();
    reader.read_line(&mut status_line).ok()?;
    let status = status_line.split_whitespace().nth(1)?;
    status.parse::<u16>().ok()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn state_server_profile_endpoint_rejects_invalid_npub() {
    let bind = format!("127.0.0.1:{}", reserve_local_port());
    let config = StateConfig {
        bind: bind.clone(),
        storage: StorageConfig {
            read_connection: "postgres://gittree:gittree@127.0.0.1:1/gittree".to_string(),
            write_connection: None,
            max_connections: 1,
            min_connections: 1,
            idle_timeout_secs: None,
            max_lifetime_secs: None,
            application_name: Some("gittree-state-http-profile-test".to_string()),
        },
        relay_urls: vec!["wss://gittr.ee".to_string()],
    };

    let server = tokio::spawn(serve(config));
    let mut ready = false;
    for _ in 0..80 {
        if request_status(&bind, "/health") == Some(200) {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(ready, "state server did not become ready in time");

    let status = request_status(&bind, "/v1/profiles/not-an-npub").expect("status");
    assert_eq!(status, 400);

    server.abort();
    let _ = server.await;
}
