use axum::body::Bytes;
use axum::http::StatusCode;
use axum::routing::any;
use axum::{Router, serve};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use gittree_config::AuthConfig;
use gittree_core::{RepoAnnouncement, RepoMapping, parse_repo_path};
use gittree_git_http::{GitHttpConfig, serve as serve_git_http};
use gittree_nostr_auth::{NIP98_KIND, Nip98Event};
use gittree_storage::{
    AnnouncementRepository, MigrationRunner, PostgresRepositories, RepoAnnouncementRecord,
    RepoMappingRecord, RepoMappingRepository, StorageConfig, migrations,
};
use reqwest::Client;
use secp256k1::{Keypair, Message, Secp256k1, SecretKey, XOnlyPublicKey};
use sha2::Digest;
use sqlx::{Connection, PgConnection};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const TEST_NPUB: &str = "npub1gjttreegkzys8jlhdnfm3qe39h2gka79cpndd0jsms5fk7tuhcnsdw56jq";

fn reserve_local_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind local port");
    let port = listener.local_addr().expect("listener addr").port();
    drop(listener);
    port
}

fn storage_config(database_url: &str) -> StorageConfig {
    StorageConfig {
        read_connection: database_url.to_string(),
        write_connection: Some(database_url.to_string()),
        max_connections: 10,
        min_connections: 1,
        idle_timeout_secs: Some(5),
        max_lifetime_secs: Some(60),
        application_name: Some("gittree-git-http-integration".to_string()),
    }
}

fn db_url_candidates() -> Vec<String> {
    let mut candidates = Vec::new();
    if let Ok(url) = std::env::var("GITTREE_STORAGE_TEST_DATABASE_URL") {
        if !url.trim().is_empty() {
            candidates.push(url);
        }
    }
    if let Ok(url) = std::env::var("GITTREE_STORAGE_READ_URL") {
        if !url.trim().is_empty() {
            candidates.push(url);
        }
    }
    candidates.push("postgres://gittree:gittree@127.0.0.1:5432/gittree".to_string());
    candidates
}

fn with_connect_timeout(url: &str) -> String {
    if url.contains("connect_timeout=") {
        return url.to_string();
    }
    if url.contains('?') {
        format!("{url}&connect_timeout=1")
    } else {
        format!("{url}?connect_timeout=1")
    }
}

fn unique_u64() -> u64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time");
    (now.as_nanos() & u128::from(u64::MAX)) as u64
}

fn unique_hex32() -> String {
    format!("{:064x}", unique_u64())
}

fn unix_timestamp() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_secs() as i64,
        Err(_) => 0,
    }
}

async fn setup_database() -> Option<String> {
    for candidate in db_url_candidates() {
        let url = with_connect_timeout(&candidate);
        let mut connection = match PgConnection::connect(&url).await {
            Ok(connection) => connection,
            Err(_) => continue,
        };
        let runner = MigrationRunner::new(migrations::core_migrations()).ok()?;
        if runner.run(&mut connection).await.is_ok() {
            return Some(url);
        }
    }
    None
}

async fn start_upstream_server() -> (tokio::task::JoinHandle<()>, String) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind upstream");
    let addr = listener.local_addr().expect("upstream addr");
    let app = Router::new().fallback(any(|| async { (StatusCode::OK, "ok") }));
    let handle = tokio::spawn(async move {
        let _ = serve(listener, app).await;
    });
    (handle, format!("http://{addr}"))
}

async fn wait_for_health(base_url: &str) {
    let client = Client::builder()
        .timeout(Duration::from_millis(500))
        .build()
        .expect("health client");
    for _ in 0..60 {
        if let Ok(response) = client.get(format!("{base_url}/health")).send().await
            && response.status() == StatusCode::OK
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("git-http server never became ready");
}

fn signed_event_with_secret(
    url: &str,
    method: &str,
    body: &Bytes,
    created_at: i64,
    secret_fill: u8,
) -> Nip98Event {
    let secp = Secp256k1::new();
    let secret_key = SecretKey::from_slice(&[secret_fill; 32]).expect("secret");
    let keypair = Keypair::from_secret_key(&secp, &secret_key);
    let (pubkey, _) = XOnlyPublicKey::from_keypair(&keypair);
    let pubkey_hex = hex::encode(pubkey.serialize());
    let mut tags = vec![
        vec!["u".to_string(), url.to_string()],
        vec!["method".to_string(), method.to_string()],
    ];
    if !body.is_empty() {
        let mut hasher = sha2::Sha256::new();
        hasher.update(body);
        tags.push(vec!["payload".to_string(), hex::encode(hasher.finalize())]);
    }
    let mut event = Nip98Event {
        id: String::new(),
        pubkey: pubkey_hex,
        created_at,
        kind: NIP98_KIND,
        tags,
        content: String::new(),
        sig: String::new(),
    };
    let payload = serde_json::json!([
        0,
        event.pubkey,
        event.created_at,
        event.kind,
        event.tags,
        event.content
    ]);
    let serialized = serde_json::to_string(&payload).expect("serialize");
    let mut hasher = sha2::Sha256::new();
    hasher.update(serialized.as_bytes());
    let digest = hasher.finalize();
    let event_id = hex::encode(digest);
    let bytes = hex::decode(&event_id).expect("decode event id");
    let msg = Message::from_digest_slice(&bytes).expect("message");
    let signature = secp.sign_schnorr_no_aux_rand(&msg, &keypair);
    event.id = event_id;
    event.sig = hex::encode(signature.as_ref());
    event
}

fn signed_event(url: &str, method: &str, body: &Bytes, created_at: i64) -> Nip98Event {
    signed_event_with_secret(url, method, body, created_at, 4)
}

async fn send_http10_without_host(
    address: &str,
    path: &str,
    authorization: &str,
    body: &Bytes,
) -> u16 {
    let mut stream = tokio::net::TcpStream::connect(address)
        .await
        .expect("connect raw stream");
    let request = format!(
        "POST {path} HTTP/1.0\r\nauthorization: {authorization}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        body.len()
    );
    stream
        .write_all(request.as_bytes())
        .await
        .expect("write request");
    stream.write_all(body).await.expect("write body");
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .await
        .expect("read response");
    let first_line = String::from_utf8_lossy(&response)
        .lines()
        .next()
        .unwrap_or_default()
        .to_string();
    first_line
        .split_whitespace()
        .nth(1)
        .and_then(|value| value.parse::<u16>().ok())
        .expect("status code")
}

#[tokio::test]
async fn git_http_serve_exercises_concrete_runtime_paths_with_postgres_backing() {
    let Some(database_url) = setup_database().await else {
        eprintln!("skipping concrete git-http runtime test: postgres unavailable");
        return;
    };

    let unique = unique_u64();
    let identifier = format!("runtime-{unique}");
    let forgejo_repo = format!("repo-{unique}");

    let repo_path = Path::new("/")
        .join(TEST_NPUB)
        .join(format!("{identifier}.git"));
    let parsed = parse_repo_path(repo_path).expect("parse repo path");

    let storage = storage_config(&database_url);
    let pool_options = storage.pool_options().expect("pool options");
    let connect_options = storage.read_connect_options().expect("connect options");
    let repositories = PostgresRepositories::new(pool_options.connect_lazy_with(connect_options));

    let mapping = RepoMapping::new("owner", &forgejo_repo, parsed.pubkey.clone(), &identifier)
        .expect("mapping");
    repositories
        .upsert_mapping(RepoMappingRecord::new(&mapping).expect("mapping record"))
        .await
        .expect("insert mapping");

    let body = Bytes::from_static(b"pkt-line");
    let bind_port = reserve_local_port();
    let base_url = format!("http://127.0.0.1:{bind_port}");
    let receive_path = format!("/{TEST_NPUB}/{identifier}.git/git-receive-pack");
    let receive_url = format!("{base_url}{receive_path}");
    let created_at = unix_timestamp();
    let event = signed_event(&receive_url, "POST", &body, created_at);

    let announcement = RepoAnnouncement {
        identifier: identifier.clone(),
        name: None,
        description: None,
        root_commit: None,
        clone: vec![format!("https://gittr.ee/owner/{forgejo_repo}.git")],
        web: Vec::new(),
        relays: vec!["wss://gittr.ee".to_string()],
        blossoms: Vec::new(),
        hashtags: Vec::new(),
        maintainers: vec![event.pubkey.clone()],
    };
    repositories
        .insert_announcement(
            RepoAnnouncementRecord::new(&unique_hex32(), &parsed.pubkey, created_at, &announcement)
                .expect("announcement record"),
        )
        .await
        .expect("insert announcement");

    let (upstream_task, upstream_url) = start_upstream_server().await;

    let config = GitHttpConfig {
        bind: format!("127.0.0.1:{bind_port}"),
        upstream_url,
        timeout: Duration::from_secs(1),
        auth: AuthConfig {
            email_domain: "example.com".to_string(),
            max_skew_seconds: 300,
        },
        storage,
    };

    let server_task = tokio::spawn(async move {
        let _ = serve_git_http(config).await;
    });

    wait_for_health(&base_url).await;

    let client = Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .expect("client");

    let missing = client
        .get(format!("{base_url}/missing"))
        .send()
        .await
        .expect("missing response");
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);

    let info_refs = client
        .get(format!(
            "{base_url}/{TEST_NPUB}/{identifier}.git/info/refs?service=git-upload-pack"
        ))
        .send()
        .await
        .expect("info refs response");
    assert_eq!(info_refs.status(), StatusCode::OK);

    let token = BASE64_STANDARD.encode(serde_json::to_vec(&event).expect("event json"));
    let receive_pack = client
        .post(receive_url.clone())
        .header("authorization", format!("Nostr {token}"))
        .body(body.clone())
        .send()
        .await
        .expect("receive-pack response");
    assert_eq!(receive_pack.status(), StatusCode::OK);

    let missing_mapping = client
        .get(format!(
            "{base_url}/{TEST_NPUB}/missing-{unique}.git/info/refs?service=git-upload-pack"
        ))
        .send()
        .await
        .expect("missing mapping response");
    assert_eq!(missing_mapping.status(), StatusCode::NOT_FOUND);

    let unauthorized_event = signed_event_with_secret(&receive_url, "POST", &body, created_at, 7);
    let unauthorized_token =
        BASE64_STANDARD.encode(serde_json::to_vec(&unauthorized_event).expect("event json"));
    let unauthorized = client
        .post(receive_url.clone())
        .header("authorization", format!("Nostr {unauthorized_token}"))
        .body(body.clone())
        .send()
        .await
        .expect("unauthorized response");
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

    let same_as_repo_announcement = RepoAnnouncement {
        identifier: identifier.clone(),
        name: None,
        description: None,
        root_commit: None,
        clone: vec![format!("https://gittr.ee/owner/{forgejo_repo}.git")],
        web: Vec::new(),
        relays: vec!["wss://gittr.ee".to_string()],
        blossoms: Vec::new(),
        hashtags: Vec::new(),
        maintainers: vec![parsed.pubkey.clone()],
    };
    repositories
        .insert_announcement(
            RepoAnnouncementRecord::new(
                &unique_hex32(),
                &parsed.pubkey,
                created_at + 1,
                &same_as_repo_announcement,
            )
            .expect("announcement record"),
        )
        .await
        .expect("insert announcement");
    let same_as_repo = client
        .post(receive_url.clone())
        .header("authorization", format!("Nostr {token}"))
        .body(body.clone())
        .send()
        .await
        .expect("same maintainer response");
    assert_eq!(same_as_repo.status(), StatusCode::UNAUTHORIZED);

    let raw_token = BASE64_STANDARD.encode(serde_json::to_vec(&event).expect("event json"));
    let raw_status = send_http10_without_host(
        &format!("127.0.0.1:{bind_port}"),
        &receive_path,
        &format!("Nostr {raw_token}"),
        &body,
    )
    .await;
    assert_eq!(raw_status, StatusCode::BAD_REQUEST.as_u16());

    server_task.abort();
    let _ = server_task.await;
    upstream_task.abort();
    let _ = upstream_task.await;
}
