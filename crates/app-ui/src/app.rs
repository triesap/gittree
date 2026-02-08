#![forbid(unsafe_code)]

use leptos::prelude::*;
use leptos::prelude::IntoAny;
use leptos_router::components::{Route, Router, Routes};
use leptos_router::hooks::use_params_map;
use leptos_router::path;
use gittree_app_core::RepoListResponse;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{Request, RequestInit, RequestMode, Response};
use crate::auth::{local_key_event, nip07_available, nip07_pubkey, nip07_sign_nip98, unix_timestamp};
use crate::auth_client::{signup, signup_endpoint, SignupResponse};
use crate::i18n::app_i18n_init;
use crate::session::{AuthSession, AuthSource, clear_session, load_session, store_session};
use crate::server::{list_repositories, repo_detail};
use crate::t;

#[derive(Clone)]
struct AppBasePath(String);

#[derive(Clone, Debug)]
enum HealthState {
    Idle,
    Ok(u16),
    Error(String),
}

#[component]
pub fn GittreeApp() -> impl IntoView {
    provide_context(app_i18n_init());
    let base_path = resolve_base_path();
    provide_context(AppBasePath(base_path.clone()));
    let auth_url = resolve_auth_url();
    let app_url = resolve_app_url();
    let control_url = resolve_control_url();

    view! {
        <Router>
            <main
                id="gittree-app"
                class="gt-app"
                data-base-path=base_path
                data-auth-url=auth_url
                data-app-url=app_url
                data-control-url=control_url
            >
                <Routes fallback=|| view! { <NotFoundPage /> }>
                    <Route path=path!("/") view=RepoListPage />
                    <Route path=path!("/signup") view=SignupPage />
                    <Route path=path!("/test") view=TestConsolePage />
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
    let test_href = test_href(&base_path);
    let repos = Resource::new(|| (), |_| list_repositories());

    view! {
        <section class="gt-panel">
            <h1 class="gt-title">{t!("app.title")}</h1>
            <p class="gt-tagline">{t!("app.tagline")}</p>
            <p class="gt-meta">
                <a class="gt-link" href=signup_href>{t!("app.signup.cta")}</a>
                " · "
                <a class="gt-link" href=test_href>{t!("app.test.title")}</a>
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
    let nip07_ready = nip07_available();
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
                    disabled=move || busy.get() || !auth_ready || !nip07_ready
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
fn TestConsolePage() -> impl IntoView {
    let base_path = app_base_path();
    let auth_url = resolve_auth_url();
    let auth_endpoint = signup_endpoint(&auth_url).unwrap_or_default();
    let auth_ready = !auth_endpoint.is_empty();
    let nip07_ready = nip07_available();
    let api_url = resolve_app_url();
    let repos_endpoint = repo_list_endpoint(&api_url);
    let auth_health_url = health_endpoint(&auth_url);
    let app_health_url = health_endpoint(&api_url);
    let control_url = resolve_control_url();
    let control_health_url = health_endpoint(&control_url);

    let (session, set_session) = signal::<Option<AuthSession>>(None);
    let (signup_status, set_signup_status) = signal::<Option<SignupResponse>>(None);
    let (repos_status, set_repos_status) = signal::<Option<RepoListResponse>>(None);
    let (auth_health, set_auth_health) = signal(HealthState::Idle);
    let (app_health, set_app_health) = signal(HealthState::Idle);
    let (control_health, set_control_health) = signal(HealthState::Idle);
    let (error, set_error) = signal::<Option<String>>(None);
    let (busy, set_busy) = signal(false);

    if let Err(err) = load_session().map(|value| set_session.set(value)) {
        set_error.set(Some(err.to_string()));
    }

    let auth_endpoint_nip07 = auth_endpoint.clone();
    let auth_endpoint_local = auth_endpoint.clone();
    let repos_endpoint_fetch = repos_endpoint.clone();
    let auth_health_endpoint = auth_health_url.clone();
    let auth_health_disabled = auth_health_url.clone();
    let app_health_endpoint = app_health_url.clone();
    let app_health_disabled = app_health_url.clone();
    let control_health_endpoint = control_health_url.clone();
    let control_health_disabled = control_health_url.clone();

    let refresh_session = move |_| match load_session() {
        Ok(value) => set_session.set(value),
        Err(err) => set_error.set(Some(err.to_string())),
    };

    let clear_session_action = move |_| {
        if let Err(err) = clear_session() {
            set_error.set(Some(err.to_string()));
        }
        set_session.set(None);
    };

    let signup_nip07 = move |_| {
        let auth_endpoint = auth_endpoint_nip07.clone();
        let set_error = set_error.clone();
        let set_busy = set_busy.clone();
        let set_signup_status = set_signup_status.clone();
        let set_session = set_session.clone();

        if auth_endpoint.is_empty() {
            set_error.set(Some(t!("app.signup.missing_auth").to_string()));
            return;
        }

        leptos::task::spawn_local(async move {
            set_busy.set(true);
            set_error.set(None);
            set_signup_status.set(None);

            let now = unix_timestamp();
            let event = match nip07_pubkey().await {
                Ok(pubkey) => nip07_sign_nip98(pubkey, "POST", &auth_endpoint, None, now).await,
                Err(err) => Err(err),
            };

            match event {
                Ok(event) => match signup(&auth_endpoint, event).await {
                    Ok(response) => {
                        match persist_session(&response.pubkey, AuthSource::Nip07) {
                            Ok(session) => set_session.set(Some(session)),
                            Err(message) => set_error.set(Some(message)),
                        }
                        set_signup_status.set(Some(response));
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
        let set_busy = set_busy.clone();
        let set_signup_status = set_signup_status.clone();
        let set_session = set_session.clone();

        if auth_endpoint.is_empty() {
            set_error.set(Some(t!("app.signup.missing_auth").to_string()));
            return;
        }

        leptos::task::spawn_local(async move {
            set_busy.set(true);
            set_error.set(None);
            set_signup_status.set(None);

            let now = unix_timestamp();
            match local_key_event("POST", &auth_endpoint, None, now) {
                Ok(event) => match signup(&auth_endpoint, event).await {
                    Ok(response) => {
                        match persist_session(&response.pubkey, AuthSource::Local) {
                            Ok(session) => set_session.set(Some(session)),
                            Err(message) => set_error.set(Some(message)),
                        }
                        set_signup_status.set(Some(response));
                    }
                    Err(err) => set_error.set(Some(err.to_string())),
                },
                Err(err) => set_error.set(Some(err.to_string())),
            }
            set_busy.set(false);
        });
    };

    let fetch_repos = move |_| {
        let endpoint = repos_endpoint_fetch.clone();
        let set_error = set_error.clone();
        let set_busy = set_busy.clone();
        let set_repos_status = set_repos_status.clone();

        leptos::task::spawn_local(async move {
            set_busy.set(true);
            set_error.set(None);
            set_repos_status.set(None);

            match fetch_repo_list(&endpoint).await {
                Ok(response) => set_repos_status.set(Some(response)),
                Err(err) => set_error.set(Some(err)),
            }

            set_busy.set(false);
        });
    };

    let list_href = base_href(&base_path);

    view! {
        <section class="gt-panel">
            <h1 class="gt-title">{t!("app.test.title")}</h1>
            <p class="gt-tagline">{t!("app.test.tagline")}</p>

            <p class="gt-meta">{t!("app.test.section.actions")}</p>
            <div class="gt-actions">
                <button
                    class="gt-button"
                    disabled=move || busy.get() || !auth_ready || !nip07_ready
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
                <button
                    class="gt-button gt-button-secondary"
                    disabled=move || busy.get()
                    on:click=fetch_repos
                >
                    {t!("app.test.actions.list_repos")}
                </button>
            </div>

            <p class="gt-meta">{t!("app.test.section.health")}</p>
            <div class="gt-actions">
                <button
                    class="gt-button gt-button-secondary"
                    disabled=move || busy.get() || auth_health_disabled.is_empty()
                    on:click=move |_| run_health_check(auth_health_endpoint.clone(), set_auth_health.clone(), set_error.clone(), set_busy.clone())
                >
                    {t!("app.test.health.auth")}
                </button>
                <button
                    class="gt-button gt-button-secondary"
                    disabled=move || busy.get() || app_health_disabled.is_empty()
                    on:click=move |_| run_health_check(app_health_endpoint.clone(), set_app_health.clone(), set_error.clone(), set_busy.clone())
                >
                    {t!("app.test.health.app")}
                </button>
                <button
                    class="gt-button gt-button-secondary"
                    disabled=move || busy.get() || control_health_disabled.is_empty()
                    on:click=move |_| run_health_check(control_health_endpoint.clone(), set_control_health.clone(), set_error.clone(), set_busy.clone())
                >
                    {t!("app.test.health.control")}
                </button>
            </div>
            <div class="gt-status">
                <p class="gt-meta">{format!("{} {}", t!("app.test.health.auth"), render_health(&auth_health.get()))}</p>
                <p class="gt-meta">{format!("{} {}", t!("app.test.health.app"), render_health(&app_health.get()))}</p>
                <p class="gt-meta">{format!("{} {}", t!("app.test.health.control"), render_health(&control_health.get()))}</p>
            </div>

            <p class="gt-meta">{t!("app.test.section.session")}</p>
            {move || match session.get() {
                Some(session) => view! {
                    <div class="gt-status">
                        <p class="gt-meta">{format!("{} {}", t!("app.signup.pubkey"), session.pubkey)}</p>
                        <p class="gt-meta">{format!("npub: {}", session.npub)}</p>
                        <p class="gt-meta">{format!("source: {:?}", session.source)}</p>
                    </div>
                }
                .into_any(),
                None => view! { <p class="gt-meta">{t!("app.test.session.none")}</p> }.into_any(),
            }}
            <div class="gt-actions">
                <button class="gt-button gt-button-secondary" on:click=refresh_session>
                    {t!("app.test.session.refresh")}
                </button>
                <button class="gt-button gt-button-secondary" on:click=clear_session_action>
                    {t!("app.test.session.clear")}
                </button>
            </div>

            {move || match signup_status.get() {
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

            <p class="gt-meta">{t!("app.test.section.repos")}</p>
            {move || match repos_status.get() {
                Some(response) => {
                    if response.items.is_empty() {
                        view! { <p class="gt-meta">{t!("app.test.repos.empty")}</p> }.into_any()
                    } else {
                        let items = response.items;
                        view! {
                            <div class="gt-status">
                                <p class="gt-meta">{format!("{} {}", t!("app.test.repos.count"), items.len())}</p>
                            </div>
                            <ul class="gt-list">
                                {items.into_iter().map(|item| {
                                    view! {
                                        <li class="gt-list-item">
                                            <div>
                                                <span class="gt-link">{item.identifier}</span>
                                            </div>
                                            <div class="gt-meta">{item.npub}</div>
                                            <div class="gt-meta">{format!("{} {}", t!("app.repo.forgejo"), item.forgejo)}</div>
                                        </li>
                                    }
                                }).collect_view()}
                            </ul>
                        }
                        .into_any()
                    }
                }
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

fn resolve_app_url() -> String {
    app_url_from_context()
        .or_else(app_url_from_dom)
        .or_else(app_url_from_location)
        .unwrap_or_default()
}

fn resolve_control_url() -> String {
    control_url_from_context()
        .or_else(control_url_from_dom)
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

#[cfg(feature = "ssr")]
fn app_url_from_context() -> Option<String> {
    use crate::AppUiState;

    use_context::<AppUiState>().map(|state| state.app_url)
}

#[cfg(not(feature = "ssr"))]
fn app_url_from_context() -> Option<String> {
    None
}

#[cfg(feature = "ssr")]
fn control_url_from_context() -> Option<String> {
    use crate::AppUiState;

    use_context::<AppUiState>().map(|state| state.control_url)
}

#[cfg(not(feature = "ssr"))]
fn control_url_from_context() -> Option<String> {
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

#[cfg(not(feature = "ssr"))]
fn app_url_from_dom() -> Option<String> {
    use leptos::prelude::window;

    let document = window().document()?;
    let element = document.get_element_by_id("gittree-app")?;
    element.get_attribute("data-app-url")
}

#[cfg(not(feature = "ssr"))]
fn app_url_from_location() -> Option<String> {
    use leptos::prelude::window;

    window().location().origin().ok()
}

#[cfg(not(feature = "ssr"))]
fn control_url_from_dom() -> Option<String> {
    use leptos::prelude::window;

    let document = window().document()?;
    let element = document.get_element_by_id("gittree-app")?;
    element.get_attribute("data-control-url")
}

#[cfg(feature = "ssr")]
fn base_path_from_dom() -> Option<String> {
    None
}

#[cfg(feature = "ssr")]
fn auth_url_from_dom() -> Option<String> {
    None
}

#[cfg(feature = "ssr")]
fn app_url_from_dom() -> Option<String> {
    None
}

#[cfg(feature = "ssr")]
fn control_url_from_dom() -> Option<String> {
    None
}

#[cfg(feature = "ssr")]
fn app_url_from_location() -> Option<String> {
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

fn test_href(base_path: &str) -> String {
    let base = base_path.trim_end_matches('/');
    if base.is_empty() || base == "/" {
        "/test".to_string()
    } else {
        format!("{base}/test")
    }
}

fn repo_list_endpoint(app_url: &str) -> String {
    let trimmed = app_url.trim();
    if trimmed.is_empty() {
        "/api/repos".to_string()
    } else {
        format!("{}/api/repos", trimmed.trim_end_matches('/'))
    }
}

fn health_endpoint(base_url: &str) -> String {
    let trimmed = base_url.trim();
    if trimmed.is_empty() {
        String::new()
    } else {
        format!("{}/health", trimmed.trim_end_matches('/'))
    }
}

fn persist_session(pubkey: &str, source: AuthSource) -> Result<AuthSession, String> {
    let session = AuthSession::from_pubkey_hex(pubkey, source)
        .map_err(|err| err.to_string())?;
    store_session(&session).map_err(|err| err.to_string())?;
    Ok(session)
}

async fn fetch_repo_list(endpoint: &str) -> Result<RepoListResponse, String> {
    let init = RequestInit::new();
    init.set_method("GET");
    init.set_mode(RequestMode::Cors);

    let request =
        Request::new_with_str_and_init(endpoint, &init).map_err(request_error)?;
    let window = web_sys::window().ok_or_else(|| "missing window".to_string())?;
    let response = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(request_error)?;
    let response: Response = response.dyn_into().map_err(request_error)?;
    let status = response.status();
    let text = response.text().map_err(request_error)?;
    let text = JsFuture::from(text).await.map_err(request_error)?;
    let body = text.as_string().unwrap_or_default();

    if (200..300).contains(&status) {
        serde_json::from_str::<RepoListResponse>(&body)
            .map_err(|err| format!("invalid response: {err}"))
    } else if body.trim().is_empty() {
        Err("request failed".to_string())
    } else {
        Err(body)
    }
}

async fn fetch_health(endpoint: &str) -> Result<u16, String> {
    let init = RequestInit::new();
    init.set_method("GET");
    init.set_mode(RequestMode::Cors);

    let request =
        Request::new_with_str_and_init(endpoint, &init).map_err(request_error)?;
    let window = web_sys::window().ok_or_else(|| "missing window".to_string())?;
    let response = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(request_error)?;
    let response: Response = response.dyn_into().map_err(request_error)?;
    let status = response.status();
    if (200..300).contains(&status) {
        Ok(status)
    } else {
        Err(format!("status {status}"))
    }
}

fn render_health(state: &HealthState) -> String {
    match state {
        HealthState::Idle => t!("app.test.health.idle").to_string(),
        HealthState::Ok(status) => format!("{} ({status})", t!("app.test.health.ok")),
        HealthState::Error(message) => format!("{} ({message})", t!("app.test.health.fail")),
    }
}

fn run_health_check(
    endpoint: String,
    set_health: WriteSignal<HealthState>,
    set_error: WriteSignal<Option<String>>,
    set_busy: WriteSignal<bool>,
) {
    if endpoint.is_empty() {
        set_health.set(HealthState::Error("missing url".to_string()));
        return;
    }

    leptos::task::spawn_local(async move {
        set_busy.set(true);
        set_error.set(None);
        match fetch_health(&endpoint).await {
            Ok(status) => set_health.set(HealthState::Ok(status)),
            Err(err) => {
                set_health.set(HealthState::Error(err.clone()));
                set_error.set(Some(err));
            }
        }
        set_busy.set(false);
    });
}

fn request_error(value: JsValue) -> String {
    value
        .as_string()
        .unwrap_or_else(|| format!("{:?}", value))
}

#[cfg(test)]
mod tests {
    use super::{health_endpoint, repo_list_endpoint, signup_href, test_href};

    #[test]
    fn repo_list_endpoint_defaults_for_empty() {
        assert_eq!(repo_list_endpoint(""), "/api/repos");
    }

    #[test]
    fn repo_list_endpoint_trims_trailing_slash() {
        assert_eq!(
            repo_list_endpoint("http://localhost:8090/"),
            "http://localhost:8090/api/repos"
        );
    }

    #[test]
    fn health_endpoint_defaults_for_empty() {
        assert_eq!(health_endpoint(""), "");
    }

    #[test]
    fn health_endpoint_trims_trailing_slash() {
        assert_eq!(
            health_endpoint("http://localhost:8090/"),
            "http://localhost:8090/health"
        );
    }

    #[test]
    fn test_href_joins_base_path() {
        assert_eq!(test_href("/ui"), "/ui/test");
        assert_eq!(test_href("/"), "/test");
    }

    #[test]
    fn signup_href_joins_base_path() {
        assert_eq!(signup_href("/ui"), "/ui/signup");
        assert_eq!(signup_href("/"), "/signup");
    }
}
