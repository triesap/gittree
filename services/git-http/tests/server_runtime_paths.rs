use std::process::{Child, Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gittree_config::AuthConfig;
use gittree_git_http::{
    GitHttpConfig, GitHttpConfigError, GitHttpError, GitHttpRequest, GitHttpRoute, GitHttpRouter,
    GitHttpService, ReqwestUpstreamClient, StorageConfigError, UpstreamError, serve as serve_git_http,
};
use gittree_storage::StorageConfig;

const TEST_NPUB: &str = "npub1gjttreegkzys8jlhdnfm3qe39h2gka79cpndd0jsms5fk7tuhcnsdw56jq";

fn reserve_local_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind local port");
    let port = listener.local_addr().expect("listener addr").port();
    drop(listener);
    port
}

fn start_git_http_server(port: u16) -> (Child, String, bool) {
    let temp_dir = std::env::temp_dir().join(format!(
        "gittree-git-http-runtime-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    std::fs::create_dir_all(&temp_dir).expect("create temp dir");

    let storage_url = std::env::var("GITTREE_STORAGE_TEST_DATABASE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty());
    let has_runtime_database = storage_url.is_some();
    let storage_url = storage_url.unwrap_or_else(|| {
        // Fall back to an unreachable local URL so routes that require storage fail fast.
        "postgres://user:pass@127.0.0.1:1/gittree".to_string()
    });

    let base_url = format!("http://127.0.0.1:{port}");
    let mut command = Command::new(env!("CARGO_BIN_EXE_gittree-git-http"));
    command
        .current_dir(&temp_dir)
        .env("GITTREE_GIT_HTTP_BIND", format!("127.0.0.1:{port}"))
        // Use a local closed port and short timeout to keep upstream failures deterministic.
        .env("GITTREE_GIT_HTTP_UPSTREAM_URL", "http://127.0.0.1:1")
        .env("GITTREE_GIT_HTTP_TIMEOUT_SECS", "1")
        .env("GITTREE_STORAGE_READ_URL", storage_url)
        .env("GITTREE_LOG_STDOUT", "false")
        .env("GITTREE_METRICS_ENABLED", "false")
        .env("GITTREE_LOG_DIR", temp_dir.join("logs"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    let child = command.spawn().expect("spawn git-http");
    (child, base_url, has_runtime_database)
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

fn fallback_storage_url() -> String {
    std::env::var("GITTREE_STORAGE_TEST_DATABASE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "postgres://user:pass@127.0.0.1:1/gittree".to_string())
}

#[tokio::test]
async fn git_http_binary_runtime_routes_cover_non_test_monomorphizations() {
    let port = reserve_local_port();
    let (mut child, base_url, has_runtime_database) = start_git_http_server(port);
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

    if has_runtime_database {
        // Exercise the concrete info/refs path when a runtime database is available.
        let valid_info_refs = client
            .get(format!(
                "{base_url}/{TEST_NPUB}/repo.git/info/refs?service=git-upload-pack"
            ))
            .send()
            .await
            .expect("valid info refs response");
        assert!(matches!(
            valid_info_refs.status(),
            reqwest::StatusCode::NOT_FOUND
                | reqwest::StatusCode::INTERNAL_SERVER_ERROR
                | reqwest::StatusCode::BAD_GATEWAY
        ));
    }

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

#[test]
fn git_http_public_surface_covers_non_test_runtime_instantiations() {
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");

    let config_missing = GitHttpConfigError::MissingEnv("MISSING");
    assert!(std::error::Error::source(&config_missing).is_none());
    let config_storage = GitHttpConfigError::Storage(StorageConfigError::MissingEnv("READ_URL"));
    assert!(std::error::Error::source(&config_storage).is_some());

    let storage_invalid = StorageConfigError::InvalidConfig("invalid".to_string());
    assert_eq!(storage_invalid.to_string(), "invalid");

    let upstream = UpstreamError::Request("upstream".to_string());
    assert_eq!(upstream.to_string(), "upstream");

    let error = GitHttpError::Upstream("boom".to_string());
    assert!(std::error::Error::source(&error).is_none());

    let router = GitHttpRouter::new();
    let info_refs_path = format!("/{TEST_NPUB}/repo.git/info/refs");
    let request = GitHttpRequest::new("GET", &info_refs_path, Some("service=git-upload-pack"));
    let route = router.route(&request);
    assert!(matches!(
        route,
        GitHttpRoute::InfoRefs {
            service: GitHttpService::UploadPack,
            ..
        }
    ));

    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(10))
        .build()
        .expect("reqwest client");
    let _upstream =
        ReqwestUpstreamClient::try_from_client_result(Ok(client)).expect("upstream client");

    let reqwest_err = runtime.block_on(async {
        reqwest::Client::new()
            .get("http://127.0.0.1:1")
            .send()
            .await
            .expect_err("expected reqwest transport error")
    });
    let err = match ReqwestUpstreamClient::try_from_client_result(Err(reqwest_err)) {
        Ok(_) => panic!("expected builder error"),
        Err(err) => err,
    };
    assert!(matches!(err, GitHttpError::Upstream(_)));
}

#[tokio::test]
async fn git_http_serve_covers_default_builder_path() {
    let storage_url = fallback_storage_url();
    let config = GitHttpConfig {
        bind: "invalid-bind".to_string(),
        upstream_url: "http://127.0.0.1:1".to_string(),
        timeout: Duration::from_secs(1),
        auth: AuthConfig {
            email_domain: "example.com".to_string(),
            max_skew_seconds: 300,
        },
        storage: StorageConfig {
            read_connection: storage_url.clone(),
            write_connection: Some(storage_url),
            max_connections: 10,
            min_connections: 1,
            idle_timeout_secs: Some(1),
            max_lifetime_secs: Some(60),
            application_name: Some("gittree-git-http-runtime".to_string()),
        },
    };

    let err = serve_git_http(config).await.expect_err("serve error");
    assert!(matches!(err, GitHttpError::Serve(_)));
}
