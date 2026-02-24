use std::process::{Child, Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const TEST_NPUB: &str = "npub1gjttreegkzys8jlhdnfm3qe39h2gka79cpndd0jsms5fk7tuhcnsdw56jq";

fn reserve_local_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind local port");
    let port = listener.local_addr().expect("listener addr").port();
    drop(listener);
    port
}

fn start_git_http_server(port: u16) -> (Child, String) {
    let temp_dir = std::env::temp_dir().join(format!(
        "gittree-git-http-runtime-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    std::fs::create_dir_all(&temp_dir).expect("create temp dir");

    let base_url = format!("http://127.0.0.1:{port}");
    let mut command = Command::new(env!("CARGO_BIN_EXE_gittree-git-http"));
    command
        .current_dir(&temp_dir)
        .env("GITTREE_GIT_HTTP_BIND", format!("127.0.0.1:{port}"))
        // Use a local closed port and short timeout to keep upstream failures deterministic.
        .env("GITTREE_GIT_HTTP_UPSTREAM_URL", "http://127.0.0.1:1")
        .env("GITTREE_GIT_HTTP_TIMEOUT_SECS", "1")
        .env(
            "GITTREE_STORAGE_READ_URL",
            "postgres://user:pass@127.0.0.1:1/gittree?connect_timeout=1",
        )
        .env("GITTREE_LOG_STDOUT", "false")
        .env("GITTREE_METRICS_ENABLED", "false")
        .env("GITTREE_LOG_DIR", temp_dir.join("logs"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    let child = command.spawn().expect("spawn git-http");
    (child, base_url)
}

async fn wait_for_health(base_url: &str) {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(500))
        .build()
        .expect("http client");
    for _ in 0..60 {
        if let Ok(response) = client.get(format!("{base_url}/health")).send().await
            && response.status() == reqwest::StatusCode::OK
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("git-http server never became ready");
}

fn stop_git_http_server(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

#[tokio::test]
async fn git_http_binary_runtime_routes_cover_non_test_monomorphizations() {
    let port = reserve_local_port();
    let (mut child, base_url) = start_git_http_server(port);
    wait_for_health(&base_url).await;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .expect("http client");

    let missing = client
        .get(format!("{base_url}/missing"))
        .send()
        .await
        .expect("missing response");
    assert_eq!(missing.status(), reqwest::StatusCode::NOT_FOUND);

    // Keep runtime route checks independent of storage/upstream availability.
    let invalid_info_refs = client
        .get(format!("{base_url}/{TEST_NPUB}/repo.git/info/refs?other=1"))
        .send()
        .await
        .expect("invalid info refs response");
    assert_eq!(invalid_info_refs.status(), reqwest::StatusCode::NOT_FOUND);

    let wrong_method_receive_pack = client
        .get(format!("{base_url}/{TEST_NPUB}/repo.git/git-receive-pack"))
        .send()
        .await
        .expect("wrong method receive-pack response");
    assert_eq!(
        wrong_method_receive_pack.status(),
        reqwest::StatusCode::NOT_FOUND
    );

    stop_git_http_server(&mut child);
}
