use std::process::Command;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

fn unique_run_dir(label: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "gittree-relay-probe-main-entry-{label}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ))
}

fn spawn_nip11_server(body: &str) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let addr = listener.local_addr().expect("local addr");
    let body = body.to_string();
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/nostr+json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let mut req = [0_u8; 1024];
        let _ = stream.read(&mut req);
        stream.write_all(response.as_bytes()).expect("write response");
        stream.flush().expect("flush response");
    });
    (format!("ws://{addr}"), handle)
}

#[test]
fn relay_probe_binary_help_exits_successfully() {
    let run_dir = unique_run_dir("help");
    std::fs::create_dir_all(&run_dir).expect("create temp run dir");

    let output = Command::new(env!("CARGO_BIN_EXE_gittree-relay-probe"))
        .current_dir(&run_dir)
        .arg("--help")
        .output()
        .expect("run relay probe binary");

    std::fs::remove_dir_all(&run_dir).ok();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("gittree-relay-probe --relay"));
}

#[test]
fn relay_probe_binary_missing_args_exits_with_error() {
    let run_dir = unique_run_dir("error");
    std::fs::create_dir_all(&run_dir).expect("create temp run dir");

    let output = Command::new(env!("CARGO_BIN_EXE_gittree-relay-probe"))
        .current_dir(&run_dir)
        .output()
        .expect("run relay probe binary");

    std::fs::remove_dir_all(&run_dir).ok();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("relay probe failed: invalid relay url: missing --relay or --all"));
}

#[test]
fn relay_probe_binary_text_mode_prints_missing_optional() {
    let run_dir = unique_run_dir("text");
    std::fs::create_dir_all(&run_dir).expect("create temp run dir");
    let (relay_url, handle) = spawn_nip11_server(r#"{"name":"relay","supported_nips":[34]}"#);

    let output = Command::new(env!("CARGO_BIN_EXE_gittree-relay-probe"))
        .current_dir(&run_dir)
        .arg("--relay")
        .arg(&relay_url)
        .output()
        .expect("run relay probe binary");

    handle.join().expect("server join");
    std::fs::remove_dir_all(&run_dir).ok();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("missing optional"));
}

#[test]
fn relay_probe_binary_active_mode_uses_secret_key_path() {
    let run_dir = unique_run_dir("active");
    std::fs::create_dir_all(&run_dir).expect("create temp run dir");
    let (relay_url, handle) = spawn_nip11_server(r#"{"name":"relay","supported_nips":[1,11,34]}"#);

    let output = Command::new(env!("CARGO_BIN_EXE_gittree-relay-probe"))
        .current_dir(&run_dir)
        .arg("--relay")
        .arg(&relay_url)
        .arg("--active")
        .arg("--secret-key")
        .arg("1111111111111111111111111111111111111111111111111111111111111111")
        .output()
        .expect("run relay probe binary");

    handle.join().expect("server join");
    std::fs::remove_dir_all(&run_dir).ok();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("relay:"));
}

#[test]
fn relay_probe_binary_store_mode_hits_storage_path_when_db_is_reachable() {
    if TcpStream::connect("127.0.0.1:5432").is_err() {
        return;
    }

    let run_dir = unique_run_dir("store");
    std::fs::create_dir_all(&run_dir).expect("create temp run dir");
    let (relay_url, handle) = spawn_nip11_server(r#"{"name":"relay","supported_nips":[1,11,34]}"#);

    let output = Command::new(env!("CARGO_BIN_EXE_gittree-relay-probe"))
        .current_dir(&run_dir)
        .env(
            "GITTREE_STORAGE_READ_URL",
            "postgres://gittree:gittree@127.0.0.1:5432/gittree",
        )
        .env("GITTREE_STORAGE_MIN_CONNECTIONS", "1")
        .env("GITTREE_STORAGE_MAX_CONNECTIONS", "10")
        .env("GITTREE_STORAGE_MAX_LIFETIME_SECS", "120")
        .arg("--relay")
        .arg(&relay_url)
        .arg("--store")
        .output()
        .expect("run relay probe binary");

    handle.join().expect("server join");
    std::fs::remove_dir_all(&run_dir).ok();

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("relay probe storage"));
    }
}
