use gittree_core::{RepoMapping, UpdateDecision};
use gittree_git_hook::{
    evaluate_pre_receive, handle_forgejo_push, handle_post_receive, parse_forgejo_push,
    parse_updates, run_hook_from_env, verify_forgejo_signature, HookConfig, HookConfigError,
    HookError, HookMode, HookServiceError, HttpPostReceiveNotifier, HttpStateFetcher,
    MappingResolver, PostReceiveNotifier, PostReceivePayload, RefUpdate, StateFetcher,
};
use hmac::Mac;
use std::error::Error;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const SAMPLE_NPUB: &str = "npub1gjttreegkzys8jlhdnfm3qe39h2gka79cpndd0jsms5fk7tuhcnsdw56jq";

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn with_env_vars<R>(vars: &[(&str, Option<&str>)], run: impl FnOnce() -> R) -> R {
    let _guard = env_lock().lock().expect("lock env");
    let previous: Vec<(&str, Option<std::ffi::OsString>)> = vars
        .iter()
        .map(|(key, _)| (*key, std::env::var_os(key)))
        .collect();

    for (key, value) in vars {
        match value {
            Some(value) => {
                // SAFETY: tests serialize environment mutation with a process-wide mutex.
                unsafe { std::env::set_var(key, value) };
            }
            None => {
                // SAFETY: tests serialize environment mutation with a process-wide mutex.
                unsafe { std::env::remove_var(key) };
            }
        }
    }

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(run));

    for (key, value) in previous {
        match value {
            Some(value) => {
                // SAFETY: tests serialize environment mutation with a process-wide mutex.
                unsafe { std::env::set_var(key, value) };
            }
            None => {
                // SAFETY: tests serialize environment mutation with a process-wide mutex.
                unsafe { std::env::remove_var(key) };
            }
        }
    }

    match result {
        Ok(value) => value,
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

fn write_updates_file(contents: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("gittree-hook-runtime-updates-{nanos}.txt"));
    std::fs::write(&path, contents).expect("write updates");
    path
}

fn repo_path() -> PathBuf {
    Path::new("/tmp").join(SAMPLE_NPUB).join("repo.git")
}

fn start_mock_http_server(
    status: &str,
    content_type: &str,
    body: &str,
) -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
    let addr = listener.local_addr().expect("server addr");
    let status = status.to_string();
    let content_type = content_type.to_string();
    let body = body.to_string();
    let handle = std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request);
            let response = format!(
                "HTTP/1.1 {status}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    });
    (format!("http://{addr}"), handle)
}

struct CollectingNotifier {
    payloads: Mutex<Vec<PostReceivePayload>>,
}

impl CollectingNotifier {
    fn new() -> Self {
        Self {
            payloads: Mutex::new(Vec::new()),
        }
    }
}

impl PostReceiveNotifier for CollectingNotifier {
    fn notify(&self, payload: PostReceivePayload) -> Result<(), HookServiceError> {
        self.payloads.lock().expect("payload lock").push(payload);
        Ok(())
    }
}

struct StaticResolver {
    mapping: RepoMapping,
}

impl MappingResolver for StaticResolver {
    fn resolve_mapping(
        &self,
        _owner: &str,
        _repo: &str,
    ) -> Result<Option<RepoMapping>, HookServiceError> {
        Ok(Some(self.mapping.clone()))
    }
}

#[test]
fn run_hook_from_env_covers_non_test_runtime_path() {
    let updates = format!(
        "{} {} refs/nostr/{}\n",
        "0".repeat(40),
        "1".repeat(40),
        "a".repeat(64)
    );
    let updates_path = write_updates_file(&updates);
    let repo_path = repo_path();

    with_env_vars(
        &[
            ("GITTREE_HOOK_MODE", Some("pre-receive")),
            ("GITTREE_STATE_URL", Some("http://127.0.0.1:8082")),
            (
                "GITTREE_HOOK_REPO_PATH",
                Some(repo_path.to_str().expect("repo path")),
            ),
            (
                "GITTREE_HOOK_STDIN_FILE",
                Some(updates_path.to_str().expect("stdin file path")),
            ),
        ],
        || {
            let config = HookConfig::from_env().expect("hook config from env");
            assert_eq!(config.mode, HookMode::PreReceive);
            let parsed = parse_updates(&updates).expect("parse updates");
            assert_eq!(parsed.len(), 1);
            run_hook_from_env(HookMode::PreReceive).expect("run hook");
        },
    );

    std::fs::remove_file(updates_path).expect("remove updates file");
}

#[test]
fn integration_http_paths_cover_post_receive_runtime_instantiations() {
    let (endpoint, handle) = start_mock_http_server("200 OK", "application/json", "{}");
    let notifier = HttpPostReceiveNotifier::new(endpoint, Duration::from_secs(1));
    let updates = vec![RefUpdate {
        old: "0".repeat(40),
        new: "1".repeat(40),
        reference: "refs/heads/main".to_string(),
    }];

    handle_post_receive(&notifier, repo_path(), &updates).expect("handle post receive");
    handle.join().expect("server join");
}

#[test]
fn integration_generic_paths_cover_evaluate_and_forgejo_push() {
    let updates = vec![RefUpdate {
        old: "0".repeat(40),
        new: "1".repeat(40),
        reference: format!("refs/nostr/{}", "b".repeat(64)),
    }];
    let fetcher = HttpStateFetcher::new("http://127.0.0.1:8082", Duration::from_secs(1));
    let decision = evaluate_pre_receive(&fetcher, repo_path(), &updates).expect("decision");
    assert!(matches!(decision, UpdateDecision::Accept));

    let payload = r#"
    {
        "ref": "refs/heads/main",
        "before": "0000000000000000000000000000000000000000",
        "after": "1111111111111111111111111111111111111111",
        "repository": {
            "name": "repo",
            "full_name": "owner/repo",
            "owner": { "username": "owner" }
        }
    }
    "#;
    let parsed = parse_forgejo_push(payload).expect("forgejo payload");
    assert_eq!(parsed.owner, "owner");
    let resolver = StaticResolver {
        mapping: RepoMapping::new("owner", "repo", "11".repeat(32), "repo").expect("mapping"),
    };
    let notifier = CollectingNotifier::new();
    handle_forgejo_push(&resolver, &notifier, payload).expect("handle forgejo push");
    assert_eq!(
        notifier.payloads.lock().expect("payload lock")[0].identifier,
        "repo"
    );
}

#[test]
fn integration_signature_path_accepts_valid_payload() {
    let secret = "secret";
    let payload = b"{\"ok\":true}";
    let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(secret.as_bytes()).expect("mac");
    mac.update(payload);
    let signature = hex::encode(mac.finalize().into_bytes());
    verify_forgejo_signature(secret, payload, &format!("sha256={signature}")).expect("signature");
}

#[test]
fn integration_error_variants_expose_expected_sources() {
    let parse_err =
        HookServiceError::Parse(gittree_git_hook::HookError::InvalidLine("line".to_string()));
    assert!(parse_err.source().is_some());

    let core_err = HookServiceError::Core("boom".to_string());
    assert!(core_err.source().is_none());
}

#[test]
fn integration_state_fetcher_covers_runtime_latest_state_path() {
    let (base_url, handle) = start_mock_http_server(
        "200 OK",
        "application/json",
        r#"{"identifier":"repo","state":{"refs/heads/main":"abc"}}"#,
    );
    let fetcher = HttpStateFetcher::new(base_url, Duration::from_secs(1));
    let state = fetcher
        .latest_state("pubkey", "repo")
        .expect("latest state call")
        .expect("state present");
    assert_eq!(state.identifier, "repo");
    assert_eq!(
        state.state.get("refs/heads/main").expect("state entry"),
        "abc"
    );
    handle.join().expect("server join");
}

#[test]
fn integration_error_traits_cover_runtime_paths() {
    let rendered = format!("{}", HookError::InvalidSignature("bad".to_string()));
    assert_eq!(rendered, "invalid signature: bad");

    with_env_vars(&[("GITTREE_STATE_BIND", Some("bad bind"))], || {
        let err = HookConfig::from_env_with_overrides(
            Some(HookMode::PreReceive),
            Some("http://127.0.0.1:8082".to_string()),
            None,
        )
        .expect_err("config error");
        assert!(matches!(err, HookConfigError::Config(_)));
        assert!(err.source().is_some());
    });
}

#[test]
fn integration_parse_forgejo_push_rejects_empty_owner_username() {
    let payload = r#"
    {
        "ref": "refs/heads/main",
        "before": "0000000000000000000000000000000000000000",
        "after": "1111111111111111111111111111111111111111",
        "repository": {
            "name": "repo",
            "owner": { "username": "" }
        }
    }
    "#;
    let err = parse_forgejo_push(payload).expect_err("invalid payload");
    match err {
        HookError::InvalidPayload(message) => {
            assert!(message.contains("repository.owner.username"));
        }
        other => panic!("unexpected error variant: {other:?}"),
    }
}
