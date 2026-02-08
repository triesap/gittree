#![forbid(unsafe_code)]

use leptos::prelude::*;
use leptos::prelude::IntoAny;
use leptos_router::components::{Route, Router, Routes};
use leptos_router::hooks::use_params_map;
use leptos_router::path;
use crate::auth::{local_key_event, nip07_pubkey, nip07_sign_nip98, unix_timestamp};
use crate::auth_client::{signup, signup_endpoint, SignupResponse};
use crate::i18n::app_i18n_init;
use crate::session::{AuthSession, AuthSource, store_session};
use crate::server::{list_repositories, repo_detail};
use crate::t;

#[derive(Clone)]
struct AppBasePath(String);

#[component]
pub fn GittreeApp() -> impl IntoView {
    provide_context(app_i18n_init());
    let base_path = resolve_base_path();
    provide_context(AppBasePath(base_path.clone()));
    let auth_url = resolve_auth_url();

    view! {
        <Router>
            <main
                id="gittree-app"
                class="gt-app"
                data-base-path=base_path
                data-auth-url=auth_url
            >
                <Routes fallback=|| view! { <NotFoundPage /> }>
                    <Route path=path!("/") view=RepoListPage />
                    <Route path=path!("/signup") view=SignupPage />
                    <Route path=path!("/:npub/:identifier") view=RepoDetailPage />
                </Routes>
            </main>
        </Router>
    }
}

#[component]
fn RepoListPage() -> impl IntoView {
    let base_path = app_base_path();
    let signup_href = signup_href(&base_path);
    let repos = Resource::new(|| (), |_| list_repositories());

    view! {
        <section class="gt-panel">
            <h1 class="gt-title">{t!("app.title")}</h1>
            <p class="gt-tagline">{t!("app.tagline")}</p>
            <p class="gt-meta">
                <a class="gt-link" href=signup_href>{t!("app.signup.cta")}</a>
            </p>
            <Suspense fallback=|| view! { <p class="gt-meta">{t!("app.repo.loading")}</p> }>
                {move || match repos.get() {
                    None => view! { <p class="gt-meta">{t!("app.repo.loading")}</p> }.into_any(),
                    Some(Ok(response)) => {
                        if response.items.is_empty() {
                            view! { <p class="gt-meta">{t!("app.repo.empty")}</p> }.into_any()
                        } else {
                            let items = response.items;
                            view! {
                                <ul class="gt-list">
                                    {items
                                        .into_iter()
                                        .map(|item| {
                                            let href = repo_href(&base_path, &item.npub, &item.identifier);
                                            view! {
                                                <li class="gt-list-item">
                                                    <div>
                                                        <a class="gt-link" href=href>{item.identifier}</a>
                                                    </div>
                                                    <div class="gt-meta">{item.npub}</div>
                                                    <div class="gt-meta">{format!("{} {}", t!("app.repo.forgejo"), item.forgejo)}</div>
                                                    <div class="gt-clone">{format!("{} {}", t!("app.repo.clone"), item.clone_url)}</div>
                                                </li>
                                            }
                                        })
                                        .collect_view()}
                                </ul>
                            }
                            .into_any()
                        }
                    }
                    Some(Err(_)) => view! { <p class="gt-meta">{t!("app.repo.error")}</p> }.into_any(),
                }}
            </Suspense>
        </section>
    }
}

#[component]
fn RepoDetailPage() -> impl IntoView {
    let base_path = app_base_path();
    let params = use_params_map();
    let detail = Resource::new(
        move || {
            let params = params.get();
            (
                params.get("npub"),
                params.get("identifier"),
            )
        },
        |(npub, identifier)| async move {
            match (npub, identifier) {
                (Some(npub), Some(identifier)) => repo_detail(npub, identifier).await,
                _ => Err(ServerFnError::new("missing route params")),
            }
        },
    );

    view! {
        <section class="gt-panel">
            <Suspense fallback=|| view! { <p class="gt-meta">{t!("app.repo.loading")}</p> }>
                {move || match detail.get() {
                    None => view! { <p class="gt-meta">{t!("app.repo.loading")}</p> }.into_any(),
                    Some(Ok(item)) => {
                        let list_href = base_href(&base_path);
                        view! {
                            <>
                                <h1 class="gt-title">{item.identifier}</h1>
                                <p class="gt-meta">{item.npub}</p>
                                <p class="gt-meta">{format!("{} {}", t!("app.repo.forgejo"), item.forgejo)}</p>
                                <p class="gt-clone">{format!("{} {}", t!("app.repo.clone"), item.clone_url)}</p>
                                <p>
                                    <a class="gt-link" href=list_href>{t!("app.repo.back")}</a>
                                </p>
                            </>
                        }
                        .into_any()
                    }
                    Some(Err(_)) => view! { <p class="gt-meta">{t!("app.repo.detail_error")}</p> }.into_any(),
                }}
            </Suspense>
        </section>
    }
}

#[component]
fn SignupPage() -> impl IntoView {
    let base_path = app_base_path();
    let auth_url = resolve_auth_url();
    let auth_endpoint = signup_endpoint(&auth_url);
    let auth_ready = auth_endpoint.is_some();
    let auth_endpoint = auth_endpoint.unwrap_or_default();
    let missing_auth_message = t!("app.signup.missing_auth").to_string();

    let (status, set_status) = signal::<Option<SignupResponse>>(None);
    let (error, set_error) = signal::<Option<String>>(None);
    let (busy, set_busy) = signal(false);

    let auth_endpoint_nip07 = auth_endpoint.clone();
    let auth_endpoint_local = auth_endpoint.clone();
    let missing_auth_nip07 = missing_auth_message.clone();
    let missing_auth_local = missing_auth_message.clone();

    let signup_nip07 = move |_| {
        let auth_endpoint = auth_endpoint_nip07.clone();
        let set_error = set_error.clone();
        let set_status = set_status.clone();
        let set_busy = set_busy.clone();
        let missing_auth_message = missing_auth_nip07.clone();

        if auth_endpoint.is_empty() {
            set_error.set(Some(missing_auth_message));
            return;
        }

        leptos::task::spawn_local(async move {
            set_busy.set(true);
            set_error.set(None);
            set_status.set(None);

            let now = unix_timestamp();
            let event = match nip07_pubkey().await {
                Ok(pubkey) => nip07_sign_nip98(pubkey, "POST", &auth_endpoint, None, now).await,
                Err(err) => Err(err),
            };

            match event {
                Ok(event) => match signup(&auth_endpoint, event).await {
                    Ok(response) => {
                        if let Err(message) = persist_session(&response.pubkey, AuthSource::Nip07)
                        {
                            set_error.set(Some(message));
                        }
                        set_status.set(Some(response));
                    }
                    Err(err) => set_error.set(Some(err.to_string())),
                },
                Err(err) => set_error.set(Some(err.to_string())),
            }
            set_busy.set(false);
        });
    };

    let signup_local = move |_| {
        let auth_endpoint = auth_endpoint_local.clone();
        let set_error = set_error.clone();
        let set_status = set_status.clone();
        let set_busy = set_busy.clone();
        let missing_auth_message = missing_auth_local.clone();

        if auth_endpoint.is_empty() {
            set_error.set(Some(missing_auth_message));
            return;
        }

        leptos::task::spawn_local(async move {
            set_busy.set(true);
            set_error.set(None);
            set_status.set(None);
            let now = unix_timestamp();
            match local_key_event("POST", &auth_endpoint, None, now) {
                Ok(event) => match signup(&auth_endpoint, event).await {
                    Ok(response) => {
                        if let Err(message) = persist_session(&response.pubkey, AuthSource::Local)
                        {
                            set_error.set(Some(message));
                        }
                        set_status.set(Some(response));
                    }
                    Err(err) => set_error.set(Some(err.to_string())),
                },
                Err(err) => set_error.set(Some(err.to_string())),
            }
            set_busy.set(false);
        });
    };

    let list_href = base_href(&base_path);

    view! {
        <section class="gt-panel">
            <h1 class="gt-title">{t!("app.signup.title")}</h1>
            <p class="gt-tagline">{t!("app.signup.tagline")}</p>
            <div class="gt-actions">
                <button
                    class="gt-button"
                    disabled=move || busy.get() || !auth_ready
                    on:click=signup_nip07
                >
                    {t!("app.signup.action_nip07")}
                </button>
                <button
                    class="gt-button gt-button-secondary"
                    disabled=move || busy.get() || !auth_ready
                    on:click=signup_local
                >
                    {t!("app.signup.action_local")}
                </button>
            </div>
            <p class="gt-meta">
                {t!("app.signup.auth")} " " {auth_url.clone()}
            </p>
            {move || {
                if busy.get() {
                    view! { <p class="gt-meta">{t!("app.signup.pending")}</p> }.into_any()
                } else {
                    ().into_any()
                }
            }}
            {move || match status.get() {
                Some(response) => view! {
                    <div class="gt-status">
                        <p class="gt-meta">{format!("{} {}", t!("app.signup.status"), response.status)}</p>
                        <p class="gt-meta">{format!("{} {}", t!("app.signup.username"), response.username)}</p>
                        <p class="gt-meta">{format!("{} {}", t!("app.signup.pubkey"), response.pubkey)}</p>
                    </div>
                }
                .into_any(),
                None => ().into_any(),
            }}
            {move || match error.get() {
                Some(message) => view! {
                    <div class="gt-error">
                        <p class="gt-meta">{format!("{} {}", t!("app.signup.error"), message)}</p>
                    </div>
                }
                .into_any(),
                None => ().into_any(),
            }}
            <p>
                <a class="gt-link" href=list_href>{t!("app.signup.back")}</a>
            </p>
        </section>
    }
}

#[component]
fn NotFoundPage() -> impl IntoView {
    view! {
        <section class="gt-panel">
            <h1 class="gt-title">{t!("app.title")}</h1>
            <p class="gt-tagline">{t!("app.tagline")}</p>
            <p class="gt-meta">{t!("app.repo.not_found")}</p>
        </section>
    }
}

fn resolve_base_path() -> String {
    let value = base_path_from_context()
        .or_else(base_path_from_dom)
        .unwrap_or_else(|| "/".to_string());
    normalize_base_path(&value)
}

fn resolve_auth_url() -> String {
    auth_url_from_context()
        .or_else(auth_url_from_dom)
        .unwrap_or_default()
}

#[cfg(feature = "ssr")]
fn base_path_from_context() -> Option<String> {
    use crate::AppUiState;

    use_context::<AppUiState>().map(|state| state.base_path)
}

#[cfg(not(feature = "ssr"))]
fn base_path_from_context() -> Option<String> {
    None
}

#[cfg(feature = "ssr")]
fn auth_url_from_context() -> Option<String> {
    use crate::AppUiState;

    use_context::<AppUiState>().map(|state| state.auth_url)
}

#[cfg(not(feature = "ssr"))]
fn auth_url_from_context() -> Option<String> {
    None
}

#[cfg(not(feature = "ssr"))]
fn base_path_from_dom() -> Option<String> {
    use leptos::prelude::window;

    let document = window().document()?;
    let element = document.get_element_by_id("gittree-app")?;
    element.get_attribute("data-base-path")
}

#[cfg(not(feature = "ssr"))]
fn auth_url_from_dom() -> Option<String> {
    use leptos::prelude::window;

    let document = window().document()?;
    let element = document.get_element_by_id("gittree-app")?;
    element.get_attribute("data-auth-url")
}

#[cfg(feature = "ssr")]
fn base_path_from_dom() -> Option<String> {
    None
}

#[cfg(feature = "ssr")]
fn auth_url_from_dom() -> Option<String> {
    None
}

fn app_base_path() -> String {
    use_context::<AppBasePath>()
        .map(|ctx| ctx.0)
        .unwrap_or_else(|| "/".to_string())
}

fn normalize_base_path(base_path: &str) -> String {
    let trimmed = base_path.trim();
    if trimmed.is_empty() || trimmed == "/" {
        return "/".to_string();
    }
    let trimmed = trimmed.trim_end_matches('/');
    if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{trimmed}")
    }
}

fn base_href(base_path: &str) -> String {
    if base_path == "/" {
        "/".to_string()
    } else {
        base_path.trim_end_matches('/').to_string()
    }
}

fn repo_href(base_path: &str, npub: &str, identifier: &str) -> String {
    let base = base_path.trim_end_matches('/');
    if base.is_empty() {
        format!("/{npub}/{identifier}")
    } else {
        format!("{base}/{npub}/{identifier}")
    }
}

fn signup_href(base_path: &str) -> String {
    let base = base_path.trim_end_matches('/');
    if base.is_empty() || base == "/" {
        "/signup".to_string()
    } else {
        format!("{base}/signup")
    }
}

fn persist_session(pubkey: &str, source: AuthSource) -> Result<(), String> {
    let session = AuthSession::from_pubkey_hex(pubkey, source)
        .map_err(|err| err.to_string())?;
    store_session(&session).map_err(|err| err.to_string())
}
