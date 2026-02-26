use gittree_app::{AppServiceConfig, serve};
use gittree_config::UiConfig;
use gittree_storage::StorageConfig;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const TCP_IO_TIMEOUT: Duration = Duration::from_secs(2);

fn reserve_local_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind local port");
    let port = listener.local_addr().expect("listener addr").port();
    drop(listener);
    port
}

fn repo_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(|path| path.parent())
        .expect("workspace root")
        .to_path_buf()
}

fn parse_status_code(response: &str) -> Option<u16> {
    let status_line = response.lines().next().unwrap_or_default();
    let code = status_line.split_whitespace().nth(1).unwrap_or_default();
    code.parse::<u16>().ok()
}

fn read_status_code(stream: &mut TcpStream) -> Option<u16> {
    let mut reader = BufReader::new(stream);
    let mut status_line = String::new();
    reader.read_line(&mut status_line).ok()?;
    parse_status_code(status_line.trim_end())
}

fn http_status(port: u16, path: &str) -> Option<u16> {
    let endpoint = format!("127.0.0.1:{port}");
    let mut stream = TcpStream::connect(&endpoint).ok()?;
    stream.set_read_timeout(Some(TCP_IO_TIMEOUT)).ok()?;
    stream.set_write_timeout(Some(TCP_IO_TIMEOUT)).ok()?;
    let request = format!("GET {path} HTTP/1.1\r\nHost: {endpoint}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).ok()?;
    read_status_code(&mut stream)
}

fn http_post_status(port: u16, path: &str, body: &str) -> Option<u16> {
    let endpoint = format!("127.0.0.1:{port}");
    let mut stream = TcpStream::connect(&endpoint).ok()?;
    stream.set_read_timeout(Some(TCP_IO_TIMEOUT)).ok()?;
    stream.set_write_timeout(Some(TCP_IO_TIMEOUT)).ok()?;
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: {endpoint}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).ok()?;
    read_status_code(&mut stream)
}

async fn wait_for_health(port: u16) {
    for _ in 0..60 {
        if http_status(port, "/health") == Some(200) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("app server never became ready");
}

async fn wait_for_status(port: u16, path: &str) -> u16 {
    for _ in 0..40 {
        if let Some(status) = http_status(port, path) {
            return status;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("did not receive HTTP status for path: {path}");
}

async fn wait_for_post_status(port: u16, path: &str, body: &str) -> u16 {
    for _ in 0..40 {
        if let Some(status) = http_post_status(port, path, body) {
            return status;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("did not receive HTTP status for post path: {path}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn serve_runtime_path_executes_non_test_instantiations() {
    let port = reserve_local_port();
    let temp_dir = std::env::temp_dir().join(format!(
        "gittree-app-serve-runtime-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    std::fs::create_dir_all(temp_dir.join("repos")).expect("create repos dir");
    std::fs::create_dir_all(temp_dir.join("logs")).expect("create logs dir");

    let config = AppServiceConfig {
        bind: format!("127.0.0.1:{port}").parse().expect("bind"),
        base_path: "/".to_string(),
        site_root: repo_root().join("crates/app-ui/dist"),
        site_pkg_dir: "pkg".to_string(),
        storage: StorageConfig {
            read_connection: "postgres://gittree:gittree@127.0.0.1:5432/gittree".to_string(),
            write_connection: None,
            max_connections: 10,
            min_connections: 2,
            idle_timeout_secs: None,
            max_lifetime_secs: None,
            application_name: Some("gittree-app-serve-runtime-tests".to_string()),
        },
        ui: UiConfig {
            repo_root: temp_dir.join("repos"),
            public_git_url: "https://gittr.ee".to_string(),
            auth_url: "http://localhost:8089".to_string(),
            app_url: format!("http://127.0.0.1:{port}"),
            control_url: "http://localhost:8088".to_string(),
        },
    };

    let serve_task = tokio::spawn(async move { serve(config).await });
    wait_for_health(port).await;
    assert_eq!(wait_for_status(port, "/missing").await, 404);
    assert_eq!(
        wait_for_status(port, "/api/users/not-a-valid-npub/repos").await,
        400
    );
    assert_eq!(
        wait_for_status(port, "/api/repos/not-a-valid-npub/repo").await,
        400
    );
    let server_fn_status =
        wait_for_post_status(port, "/api/nonexistent_server_fn", "{}").await;
    assert_ne!(server_fn_status, 404);
    assert_ne!(server_fn_status, 405);
    serve_task.abort();
    let join_err = serve_task.await.expect_err("serve task should abort");
    assert!(join_err.is_cancelled());
}
