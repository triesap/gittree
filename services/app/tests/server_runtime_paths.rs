use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const INVALID_NPUB: &str = "not-a-valid-npub";

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

fn runtime_storage_url() -> String {
    std::env::var("GITTREE_STORAGE_TEST_DATABASE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "postgres://gittree:gittree@127.0.0.1:5432/gittree".to_string())
}

fn start_app_server(port: u16) -> (Child, String) {
    let temp_dir = std::env::temp_dir().join(format!(
        "gittree-app-runtime-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    std::fs::create_dir_all(temp_dir.join("repos")).expect("create temp repos dir");

    let app_base_url = format!("http://127.0.0.1:{port}");
    let mut command = Command::new(env!("CARGO_BIN_EXE_gittree-app"));
    command
        .current_dir(repo_root())
        .env("GITTREE_APP_BIND", format!("127.0.0.1:{port}"))
        .env("GITTREE_APP_BASE_PATH", "/")
        .env(
            "GITTREE_APP_SITE_ROOT",
            repo_root().join("crates/app-ui/dist"),
        )
        .env(
            "GITTREE_STORAGE_READ_URL",
            runtime_storage_url(),
        )
        .env("GITTREE_UI_REPO_ROOT", temp_dir.join("repos"))
        .env("GITTREE_UI_PUBLIC_GIT_URL", "https://gittr.ee")
        .env("GITTREE_UI_AUTH_URL", "http://localhost:8089")
        .env("GITTREE_UI_CONTROL_URL", "http://localhost:8088")
        .env("GITTREE_LOG_STDOUT", "false")
        .env("GITTREE_METRICS_ENABLED", "false")
        .env("GITTREE_LOG_DIR", temp_dir.join("logs"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    let child = command.spawn().expect("spawn app");
    (child, app_base_url)
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

fn http_status(base_url: &str, path: &str) -> Option<u16> {
    let endpoint = base_url.trim_start_matches("http://");
    let mut stream = TcpStream::connect(endpoint).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .ok()?;
    let request = format!("GET {path} HTTP/1.1\r\nHost: {endpoint}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).ok()?;
    read_status_code(&mut stream)
}

fn wait_for_health(base_url: &str, child: &mut Child) {
    for _ in 0..60 {
        if http_status(base_url, "/health") == Some(200) {
            return;
        }
        if let Some(status) = child.try_wait().expect("check child status") {
            let mut stderr = String::new();
            if let Some(mut stderr_pipe) = child.stderr.take() {
                let _ = stderr_pipe.read_to_string(&mut stderr);
            }
            panic!("app server exited early ({status}): {stderr}");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("app server never became ready");
}

fn stop_app_server(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

#[tokio::test]
async fn app_binary_runtime_routes_cover_non_test_monomorphizations() {
    let port = reserve_local_port();
    let (mut child, base_url) = start_app_server(port);
    wait_for_health(&base_url, &mut child);

    assert_eq!(http_status(&base_url, "/missing"), Some(404));
    assert_eq!(
        http_status(&base_url, &format!("/api/users/{INVALID_NPUB}/repos"),),
        Some(400)
    );
    assert_eq!(
        http_status(&base_url, &format!("/api/repos/{INVALID_NPUB}/repo")),
        Some(400)
    );
    stop_app_server(&mut child);
}
