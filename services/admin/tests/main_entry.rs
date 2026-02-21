use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::Command;

fn start_mock_http_server(status: &str, body: &str) -> (String, std::thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind server");
    let addr = listener.local_addr().expect("addr");
    let status = status.to_string();
    let body = body.to_string();
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let mut buf = [0u8; 4096];
        let bytes = stream.read(&mut buf).expect("read request");
        let request = String::from_utf8_lossy(&buf[..bytes]).to_string();
        let response = format!(
            "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        stream
            .write_all(response.as_bytes())
            .expect("write response");
        stream.flush().expect("flush response");
        request
    });
    (format!("http://{addr}"), handle)
}

fn run_admin(
    args: &[&str],
    control_url: Option<&str>,
    extra_env: &[(&str, &str)],
) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_gittree-admin"));
    command.current_dir("/tmp");
    for key in [
        "GITTREE_CONTROL_URL",
        "GITTREE_CONTROL_TOKEN",
        "GITTREE_STORAGE_READ_URL",
        "GITTREE_STORAGE_WRITE_URL",
    ] {
        command.env_remove(key);
    }
    if let Some(control_url) = control_url {
        command
            .env("GITTREE_CONTROL_URL", control_url)
            .env("GITTREE_CONTROL_TOKEN", "token");
    }
    for (key, value) in extra_env {
        command.env(key, value);
    }
    command.args(args).output().expect("run admin binary")
}

#[test]
fn admin_binary_help_exits_successfully() {
    let output = run_admin(&["--help"], None, &[]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("gittree-admin <command> [options]"));
}

#[test]
fn admin_binary_create_user_uses_control_endpoint() {
    let (base_url, handle) = start_mock_http_server(
        "200 OK",
        r#"{"username":"alice","email":"alice@example.com"}"#,
    );
    let output = run_admin(
        &[
            "create-user",
            "--username",
            "alice",
            "--email",
            "alice@example.com",
            "--password",
            "secret",
        ],
        Some(&base_url),
        &[],
    );
    assert!(output.status.success());
    let request = handle.join().expect("request");
    assert!(request.contains("POST /control/users"));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("created user alice"));
}

#[test]
fn admin_binary_create_org_uses_control_endpoint() {
    let (base_url, handle) =
        start_mock_http_server("200 OK", r#"{"name":"acme","full_name":"Acme Org"}"#);
    let output = run_admin(
        &["create-org", "--owner", "alice", "--name", "acme"],
        Some(&base_url),
        &[],
    );
    assert!(output.status.success());
    let request = handle.join().expect("request");
    assert!(request.contains("POST /control/orgs"));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("created org acme"));
}

#[test]
fn admin_binary_create_repo_uses_control_endpoint() {
    let (base_url, handle) = start_mock_http_server(
        "200 OK",
        r#"{"owner":"alice","name":"repo","html_url":"http://example/repo"}"#,
    );
    let output = run_admin(
        &["create-repo", "--owner", "alice", "--name", "repo"],
        Some(&base_url),
        &[],
    );
    assert!(output.status.success());
    let request = handle.join().expect("request");
    assert!(request.contains("POST /control/repos"));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("created repo repo"));
}

#[test]
fn admin_binary_create_pull_uses_control_endpoint() {
    let (base_url, handle) = start_mock_http_server(
        "200 OK",
        r#"{"number":1,"url":"http://example/pulls/1","html_url":"http://example/pulls/1"}"#,
    );
    let output = run_admin(
        &[
            "create-pull",
            "--owner",
            "alice",
            "--repo",
            "repo",
            "--head",
            "feature",
            "--base",
            "main",
            "--title",
            "my pull",
        ],
        Some(&base_url),
        &[],
    );
    assert!(output.status.success());
    let request = handle.join().expect("request");
    assert!(request.contains("POST /control/pulls"));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("created pull #1"));
}

#[test]
fn admin_binary_map_rejects_invalid_forgejo_repo() {
    let output = run_admin(
        &[
            "map",
            "--forgejo",
            "invalid",
            "--pubkey",
            "11f355bdcb7cc0af728ef3cceb9615d90684bb5b2ca5f859ab0f0b704075871a",
            "--identifier",
            "repo",
        ],
        None,
        &[],
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("gittree-admin failed: admin mapping error:"));
}

#[test]
fn admin_binary_map_reports_invalid_write_connection_before_db_connect() {
    let output = run_admin(
        &[
            "map",
            "--forgejo",
            "alice/repo",
            "--pubkey",
            "11f355bdcb7cc0af728ef3cceb9615d90684bb5b2ca5f859ab0f0b704075871a",
            "--identifier",
            "repo",
        ],
        None,
        &[
            (
                "GITTREE_STORAGE_READ_URL",
                "postgres://user:pass@localhost:5432/gittree",
            ),
            ("GITTREE_STORAGE_WRITE_URL", "invalid-write-url"),
        ],
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("gittree-admin failed: admin storage error:"));
}
