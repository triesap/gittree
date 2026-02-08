#![forbid(unsafe_code)]

use leptos::prelude::*;
use leptos::prelude::IntoAny;
use leptos_router::components::{Route, Router, Routes};
use leptos_router::hooks::use_params_map;
use leptos_router::path;
use gittree_app_core::{
    nip98_payload_hash, Nip98Event, Profile, ProfileUpdate, ProfileVisibility, RepoListResponse,
};
use js_sys::Reflect;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{Request, RequestInit, RequestMode, Response};
use crate::auth::{
    AuthError,
    local_key_event,
    local_key_material,
    nip07_available,
    nip07_pubkey,
    nip07_sign_nip98,
    unix_timestamp,
};
use crate::auth_client::{signup, signup_endpoint, SignupResponse};
use crate::control_client::{create_repo, ControlRepoInput, ControlRepoResponse};
use crate::control_token::{clear_control_token, load_control_token, store_control_token};
use crate::i18n::app_i18n_init;
use crate::profile_client::{
    fetch_profile, fetch_public_profile, profile_endpoint, public_profile_endpoint,
    update_profile,
};
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
                    <Route path=path!("/profile") view=ProfilePage />
                    <Route path=path!("/account") view=AccountPage />
                    <Route path=path!("/u/:npub") view=PublicProfilePage />
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
    let profile_href = profile_href(&base_path);
    let account_href = account_href(&base_path);
    let test_href = test_href(&base_path);
    let repos = Resource::new(|| (), |_| list_repositories());

    view! {
        <section class="gt-panel">
            <h1 class="gt-title">{t!("app.title")}</h1>
            <p class="gt-tagline">{t!("app.tagline")}</p>
            <p class="gt-meta">
                <a class="gt-link" href=signup_href>{t!("app.signup.cta")}</a>
                " · "
                <a class="gt-link" href=profile_href>{t!("app.profile.cta")}</a>
                " · "
                <a class="gt-link" href=account_href>{t!("app.account.cta")}</a>
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
fn ProfilePage() -> impl IntoView {
    let base_path = app_base_path();
    let signup_href = signup_href(&base_path);
    let auth_url = resolve_auth_url();
    let auth_endpoint = profile_endpoint(&auth_url);
    let auth_ready = auth_endpoint.is_some();
    let auth_endpoint = auth_endpoint.unwrap_or_default();

    let (session, set_session) = signal::<Option<AuthSession>>(None);
    let (profile, set_profile) = signal::<Option<Profile>>(None);
    let (status, set_status) = signal::<Option<String>>(None);
    let (error, set_error) = signal::<Option<String>>(None);
    let (busy, set_busy) = signal(false);

    let (display_name, set_display_name) = signal(String::new());
    let (bio, set_bio) = signal(String::new());
    let (avatar_url, set_avatar_url) = signal(String::new());
    let (website_url, set_website_url) = signal(String::new());
    let (location, set_location) = signal(String::new());
    let (visibility_public, set_visibility_public) = signal(false);

    if let Err(err) = load_session().map(|value| set_session.set(value)) {
        set_error.set(Some(err.to_string()));
    }

    create_effect(move |_| {
        if let Some(profile) = profile.get() {
            set_display_name.set(profile.display_name.unwrap_or_default());
            set_bio.set(profile.bio.unwrap_or_default());
            set_avatar_url.set(profile.avatar_url.unwrap_or_default());
            set_website_url.set(profile.website_url.unwrap_or_default());
            set_location.set(profile.location.unwrap_or_default());
            set_visibility_public.set(profile.visibility == ProfileVisibility::Public);
        }
    });

    let fetch_profile_action = Callback::new({
        let auth_endpoint = auth_endpoint.clone();
        let set_profile = set_profile.clone();
        let set_error = set_error.clone();
        let set_status = set_status.clone();
        let set_busy = set_busy.clone();
        move |()| {
            if auth_endpoint.is_empty() {
                set_error.set(Some(t!("app.profile.missing_auth").to_string()));
                return;
            }
            let Some(session) = session.get() else {
                return;
            };
            let auth_endpoint = auth_endpoint.clone();
            let set_profile = set_profile.clone();
            let set_error = set_error.clone();
            let set_status = set_status.clone();
            let set_busy = set_busy.clone();
            leptos::task::spawn_local(async move {
                set_busy.set(true);
                set_error.set(None);
                set_status.set(Some(t!("app.profile.loading").to_string()));

                let now = unix_timestamp();
                let event =
                    session_sign_nip98(&session, "GET", &auth_endpoint, None, now).await;
                match event {
                    Ok(event) => match fetch_profile(&auth_endpoint, event).await {
                        Ok(profile) => {
                            set_profile.set(Some(profile));
                            set_status.set(Some(t!("app.profile.loaded").to_string()));
                        }
                        Err(err) => set_error.set(Some(err.to_string())),
                    },
                    Err(err) => set_error.set(Some(err.to_string())),
                }
                set_busy.set(false);
            });
        }
    });

    let fetch_profile_on_mount = fetch_profile_action;
    create_effect(move |_| {
        if profile.get().is_none() && session.get().is_some() && auth_ready && !busy.get() {
            fetch_profile_on_mount.run(());
        }
    });

    let save_profile_action = Callback::new({
        let auth_endpoint = auth_endpoint.clone();
        let set_profile = set_profile.clone();
        let set_error = set_error.clone();
        let set_status = set_status.clone();
        let set_busy = set_busy.clone();
        move |()| {
            if auth_endpoint.is_empty() {
                set_error.set(Some(t!("app.profile.missing_auth").to_string()));
                return;
            }
            let Some(session) = session.get() else {
                set_error.set(Some(t!("app.profile.missing_session").to_string()));
                return;
            };
            let auth_endpoint = auth_endpoint.clone();
            let set_profile = set_profile.clone();
            let set_error = set_error.clone();
            let set_status = set_status.clone();
            let set_busy = set_busy.clone();
            let update = ProfileUpdate {
                display_name: Some(display_name.get()),
                bio: Some(bio.get()),
                avatar_url: Some(avatar_url.get()),
                website_url: Some(website_url.get()),
                location: Some(location.get()),
                visibility: Some(if visibility_public.get() {
                    ProfileVisibility::Public
                } else {
                    ProfileVisibility::Private
                }),
            };
            leptos::task::spawn_local(async move {
                set_busy.set(true);
                set_error.set(None);
                set_status.set(Some(t!("app.profile.saving").to_string()));
                let body = match serde_json::to_vec(&update) {
                    Ok(body) => body,
                    Err(err) => {
                        set_error.set(Some(err.to_string()));
                        set_busy.set(false);
                        return;
                    }
                };
                let payload_hash = nip98_payload_hash(&body);
                let payload_hash = match payload_hash {
                    Some(hash) => hash,
                    None => {
                        set_error.set(Some("missing payload hash".to_string()));
                        set_busy.set(false);
                        return;
                    }
                };
                let now = unix_timestamp();
                let event = session_sign_nip98(
                    &session,
                    "PATCH",
                    &auth_endpoint,
                    Some(&payload_hash),
                    now,
                )
                .await;
                match event {
                    Ok(event) => match update_profile(&auth_endpoint, event, body).await {
                        Ok(profile) => {
                            set_profile.set(Some(profile));
                            set_status.set(Some(t!("app.profile.updated").to_string()));
                        }
                        Err(err) => set_error.set(Some(err.to_string())),
                    },
                    Err(err) => set_error.set(Some(err.to_string())),
                }
                set_busy.set(false);
            });
        }
    });

    view! {
        <section class="gt-panel">
            <h1 class="gt-title">{t!("app.profile.title")}</h1>
            <p class="gt-tagline">{t!("app.profile.tagline")}</p>
            {move || match session.get() {
                None => {
                    view! {
                        <p class="gt-meta">
                            {t!("app.profile.missing_session")}
                            " "
                            <a class="gt-link" href=signup_href.clone()>{t!("app.signup.cta")}</a>
                        </p>
                    }
                        .into_any()
                }
                Some(session) => {
                    view! {
                        <div class="gt-meta">
                            {format!("{} {}", t!("app.profile.pubkey"), session.pubkey)}
                        </div>
                        <div class="gt-meta">
                            {format!("{} {}", t!("app.profile.npub"), session.npub)}
                        </div>
                        <div class="gt-meta">
                            <a class="gt-link" href=public_profile_href(&base_path, &session.npub)>
                                {t!("app.profile.public.cta")}
                            </a>
                        </div>
                        <form class="gt-form">
                            <div class="gt-field">
                                <label class="gt-label">{t!("app.profile.display_name")}</label>
                                <input
                                    class="gt-input"
                                    type="text"
                                    prop:value=display_name
                                    on:input=move |ev| set_display_name.set(event_target_value(&ev))
                                />
                            </div>
                            <div class="gt-field">
                                <label class="gt-label">{t!("app.profile.bio")}</label>
                                <textarea
                                    class="gt-input gt-textarea"
                                    prop:value=bio
                                    on:input=move |ev| set_bio.set(event_target_value(&ev))
                                />
                            </div>
                            <div class="gt-field">
                                <label class="gt-label">{t!("app.profile.avatar_url")}</label>
                                <input
                                    class="gt-input"
                                    type="text"
                                    prop:value=avatar_url
                                    on:input=move |ev| set_avatar_url.set(event_target_value(&ev))
                                />
                            </div>
                            <div class="gt-field">
                                <label class="gt-label">{t!("app.profile.website_url")}</label>
                                <input
                                    class="gt-input"
                                    type="text"
                                    prop:value=website_url
                                    on:input=move |ev| set_website_url.set(event_target_value(&ev))
                                />
                            </div>
                            <div class="gt-field">
                                <label class="gt-label">{t!("app.profile.location")}</label>
                                <input
                                    class="gt-input"
                                    type="text"
                                    prop:value=location
                                    on:input=move |ev| set_location.set(event_target_value(&ev))
                                />
                            </div>
                            <div class="gt-field">
                                <label class="gt-label">{t!("app.profile.visibility")}</label>
                                <label class="gt-inline">
                                    <input
                                        class="gt-checkbox"
                                        type="checkbox"
                                        prop:checked=visibility_public
                                        on:change=move |ev| {
                                            set_visibility_public.set(event_target_checked(&ev))
                                        }
                                    />
                                    <span>{t!("app.profile.visibility_public")}</span>
                                </label>
                            </div>
                        </form>
                        <div class="gt-actions">
                            <button
                                class="gt-button"
                                type="button"
                                on:click=move |_| save_profile_action.run(())
                                disabled=move || busy.get()
                            >
                                {t!("app.profile.save")}
                            </button>
                            <button
                                class="gt-button gt-button-secondary"
                                type="button"
                                on:click=move |_| fetch_profile_action.run(())
                                disabled=move || busy.get()
                            >
                                {t!("app.profile.load")}
                            </button>
                        </div>
                    }
                        .into_any()
                }
            }}
            {move || match status.get() {
                None => ().into_any(),
                Some(message) => view! { <div class="gt-status">{message}</div> }.into_any(),
            }}
            {move || match error.get() {
                None => ().into_any(),
                Some(message) => {
                    view! { <div class="gt-error">{format!("{} {}", t!("app.profile.error"), message)}</div> }
                        .into_any()
                }
            }}
        </section>
    }
}

#[component]
fn AccountPage() -> impl IntoView {
    let base_path = app_base_path();
    let signup_href = signup_href(&base_path);
    let profile_href = profile_href(&base_path);
    let (session, set_session) = signal::<Option<AuthSession>>(None);
    let (error, set_error) = signal::<Option<String>>(None);

    if let Err(err) = load_session().map(|value| set_session.set(value)) {
        set_error.set(Some(err.to_string()));
    }

    let refresh_action = move |_| match load_session() {
        Ok(value) => set_session.set(value),
        Err(err) => set_error.set(Some(err.to_string())),
    };

    let clear_action = move |_| {
        if let Err(err) = clear_session() {
            set_error.set(Some(err.to_string()));
            return;
        }
        set_session.set(None);
    };

    view! {
        <section class="gt-panel">
            <h1 class="gt-title">{t!("app.account.title")}</h1>
            <p class="gt-tagline">{t!("app.account.tagline")}</p>
            {move || match session.get() {
                None => view! {
                    <p class="gt-meta">
                        {t!("app.account.none")}
                        " "
                        <a class="gt-link" href=signup_href.clone()>{t!("app.signup.cta")}</a>
                    </p>
                }
                .into_any(),
                Some(session) => {
                    let public_href = public_profile_href(&base_path, &session.npub);
                    view! {
                        <div class="gt-meta">
                            {format!("{} {}", t!("app.account.pubkey"), session.pubkey)}
                        </div>
                        <div class="gt-meta">
                            {format!("{} {}", t!("app.account.npub"), session.npub)}
                        </div>
                        <div class="gt-meta">
                            {format!("{} {}", t!("app.account.source"), auth_source_label(session.source))}
                        </div>
                        <div class="gt-meta">
                            <a class="gt-link" href=profile_href.clone()>{t!("app.profile.cta")}</a>
                            " · "
                            <a class="gt-link" href=public_href>{t!("app.profile.public.cta")}</a>
                        </div>
                    }
                    .into_any()
                }
            }}
            <div class="gt-actions">
                <button class="gt-button" type="button" on:click=refresh_action>
                    {t!("app.account.refresh")}
                </button>
                <button class="gt-button gt-button-secondary" type="button" on:click=clear_action>
                    {t!("app.account.clear")}
                </button>
            </div>
            {move || match error.get() {
                None => ().into_any(),
                Some(message) => {
                    view! { <div class="gt-error">{format!("{} {}", t!("app.account.error"), message)}</div> }
                        .into_any()
                }
            }}
        </section>
    }
}

#[component]
fn PublicProfilePage() -> impl IntoView {
    let base_path = app_base_path();
    let auth_url = resolve_auth_url();
    let app_url = resolve_app_url();
    let params = use_params_map();
    let list_href = base_href(&base_path);

    let (profile, set_profile) = signal::<Option<Result<Profile, String>>>(None);
    let (repos, set_repos) = signal::<Option<Result<RepoListResponse, String>>>(None);
    let (loading, set_loading) = signal(false);
    let missing_auth_message = t!("app.profile.missing_auth").to_string();

    create_effect(move |_| {
        let npub = params.get().get("npub").unwrap_or_default();
        let auth_url = auth_url.clone();
        let app_url = app_url.clone();
        let set_profile = set_profile.clone();
        let set_repos = set_repos.clone();
        let set_loading = set_loading.clone();
        let missing_auth_message = missing_auth_message.clone();

        set_profile.set(None);
        set_repos.set(None);

        if npub.trim().is_empty() {
            set_profile.set(Some(Err("missing npub".to_string())));
            set_repos.set(Some(Err("missing npub".to_string())));
            return;
        }

        set_loading.set(true);
        leptos::task::spawn_local(async move {
            let profile_result = match public_profile_endpoint(&auth_url, &npub) {
                Some(endpoint) => fetch_public_profile(&endpoint)
                    .await
                    .map_err(|err| err.to_string()),
                None => Err(missing_auth_message),
            };
            set_profile.set(Some(profile_result));

            let repos_result = {
                let endpoint = repo_list_by_owner_endpoint(&app_url, &npub);
                fetch_repo_list(&endpoint).await
            };
            set_repos.set(Some(repos_result));
            set_loading.set(false);
        });
    });

    view! {
        <section class="gt-panel">
            <h1 class="gt-title">{t!("app.profile.public.title")}</h1>
            <p class="gt-tagline">{t!("app.profile.public.tagline")}</p>
            {move || {
                if loading.get() && profile.get().is_none() {
                    return view! { <p class="gt-meta">{t!("app.profile.loading")}</p> }.into_any();
                }
                match profile.get() {
                    None => ().into_any(),
                    Some(Ok(profile)) => {
                        let display_name =
                            profile.display_name.clone().unwrap_or_else(|| profile.username.clone());
                        let npub_label = params.get().get("npub").unwrap_or_default();
                        view! {
                            <>
                                <h2 class="gt-title">{display_name}</h2>
                                <p class="gt-meta">{format!("@{}", profile.username)}</p>
                                <p class="gt-meta">{format!("{} {}", t!("app.profile.npub"), npub_label)}</p>
                                {move || match profile.bio.clone() {
                                    None => ().into_any(),
                                    Some(bio) => view! { <p class="gt-meta">{bio}</p> }.into_any(),
                                }}
                            </>
                        }
                        .into_any()
                    }
                    Some(Err(message)) => view! {
                        <p class="gt-meta">{format!("{} {}", t!("app.profile.public.error"), message)}</p>
                    }
                    .into_any(),
                }
            }}
            <h3 class="gt-meta">{t!("app.profile.public.repos")}</h3>
            {move || {
                if loading.get() && repos.get().is_none() {
                    return view! { <p class="gt-meta">{t!("app.repo.loading")}</p> }.into_any();
                }
                match repos.get() {
                    None => ().into_any(),
                    Some(Ok(response)) => {
                        if response.items.is_empty() {
                            view! { <p class="gt-meta">{t!("app.profile.public.repos_empty")}</p> }
                                .into_any()
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
                                                </li>
                                            }
                                        })
                                        .collect_view()}
                                </ul>
                            }
                            .into_any()
                        }
                    }
                    Some(Err(message)) => view! {
                        <p class="gt-meta">{format!("{} {}", t!("app.profile.public.error"), message)}</p>
                    }
                    .into_any(),
                }
            }}
            <p>
                <a class="gt-link" href=list_href>{t!("app.repo.back")}</a>
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
    let (control_token, set_control_token) = signal(String::new());
    let (control_repo_name, set_control_repo_name) = signal(String::new());
    let (control_repo_identifier, set_control_repo_identifier) = signal(String::new());
    let (control_repo_owner, set_control_repo_owner) = signal(String::new());
    let (control_repo_description, set_control_repo_description) = signal(String::new());
    let (control_repo_private, set_control_repo_private) = signal(true);
    let (control_repo_status, set_control_repo_status) =
        signal::<Option<ControlRepoResponse>>(None);
    let (control_repo_pubkey, set_control_repo_pubkey) =
        signal::<Option<String>>(None);
    let (error, set_error) = signal::<Option<String>>(None);
    let (busy, set_busy) = signal(false);

    if let Err(err) = load_session().map(|value| set_session.set(value)) {
        set_error.set(Some(err.to_string()));
    }
    match load_control_token() {
        Ok(Some(token)) => set_control_token.set(token),
        Ok(None) => {}
        Err(err) => set_error.set(Some(err.to_string())),
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
    let control_url_for_repo = control_url.clone();

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

    let save_control_token_action = move |_| {
        if let Err(err) = store_control_token(&control_token.get()) {
            set_error.set(Some(err.to_string()));
        }
    };

    let clear_control_token_action = move |_| {
        if let Err(err) = clear_control_token() {
            set_error.set(Some(err.to_string()));
            return;
        }
        set_control_token.set(String::new());
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

    let create_repo_action = move |_| {
        let control_url = control_url_for_repo.clone();
        let token = control_token.get();
        let name = control_repo_name.get();
        let identifier = control_repo_identifier.get();
        let owner = control_repo_owner.get();
        let description = control_repo_description.get();
        let private = control_repo_private.get();
        let set_error = set_error.clone();
        let set_busy = set_busy.clone();
        let set_repo_status = set_control_repo_status.clone();
        let set_repo_pubkey = set_control_repo_pubkey.clone();

        if token.trim().is_empty() {
            set_error.set(Some(t!("app.test.control.missing_token").to_string()));
            return;
        }
        if name.trim().is_empty() {
            set_error.set(Some(t!("app.test.control.missing_name").to_string()));
            return;
        }

        leptos::task::spawn_local(async move {
            set_busy.set(true);
            set_error.set(None);
            set_repo_status.set(None);
            set_repo_pubkey.set(None);

            let material = match local_key_material() {
                Ok(material) => material,
                Err(err) => {
                    set_error.set(Some(err.to_string()));
                    set_busy.set(false);
                    return;
                }
            };

            let input = ControlRepoInput {
                name: name.trim().to_string(),
                owner: if owner.trim().is_empty() {
                    None
                } else {
                    Some(owner.trim().to_string())
                },
                identifier: if identifier.trim().is_empty() {
                    None
                } else {
                    Some(identifier.trim().to_string())
                },
                description: if description.trim().is_empty() {
                    None
                } else {
                    Some(description.trim().to_string())
                },
                private: Some(private),
                pubkey: material.pubkey.clone(),
                privkey: material.privkey,
            };

            match create_repo(&control_url, &token, input).await {
                Ok(repo) => {
                    set_repo_pubkey.set(Some(material.pubkey));
                    set_repo_status.set(Some(repo));
                }
                Err(err) => set_error.set(Some(err.to_string())),
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

            <p class="gt-meta">{t!("app.test.section.control")}</p>
            <div class="gt-form">
                <div class="gt-field">
                    <label class="gt-label">{t!("app.test.control.token")}</label>
                    <input
                        class="gt-input"
                        type="password"
                        placeholder=t!("app.test.control.token.placeholder")
                        value=move || control_token.get()
                        on:input=move |ev| set_control_token.set(event_value(&ev))
                    />
                </div>
                <div class="gt-actions">
                    <button
                        class="gt-button gt-button-secondary"
                        disabled=move || busy.get()
                        on:click=save_control_token_action
                    >
                        {t!("app.test.control.token.save")}
                    </button>
                    <button
                        class="gt-button gt-button-secondary"
                        disabled=move || busy.get()
                        on:click=clear_control_token_action
                    >
                        {t!("app.test.control.token.clear")}
                    </button>
                </div>
            </div>
            <div class="gt-form">
                <div class="gt-field">
                    <label class="gt-label">{t!("app.test.control.repo.name")}</label>
                    <input
                        class="gt-input"
                        type="text"
                        placeholder=t!("app.test.control.repo.name_placeholder")
                        value=move || control_repo_name.get()
                        on:input=move |ev| set_control_repo_name.set(event_value(&ev))
                    />
                </div>
                <div class="gt-field">
                    <label class="gt-label">{t!("app.test.control.repo.identifier")}</label>
                    <input
                        class="gt-input"
                        type="text"
                        placeholder=t!("app.test.control.repo.identifier_placeholder")
                        value=move || control_repo_identifier.get()
                        on:input=move |ev| set_control_repo_identifier.set(event_value(&ev))
                    />
                </div>
                <div class="gt-field">
                    <label class="gt-label">{t!("app.test.control.repo.owner")}</label>
                    <input
                        class="gt-input"
                        type="text"
                        placeholder=t!("app.test.control.repo.owner_placeholder")
                        value=move || control_repo_owner.get()
                        on:input=move |ev| set_control_repo_owner.set(event_value(&ev))
                    />
                </div>
                <div class="gt-field">
                    <label class="gt-label">{t!("app.test.control.repo.description")}</label>
                    <textarea
                        class="gt-input gt-textarea"
                        placeholder=t!("app.test.control.repo.description_placeholder")
                        prop:value=move || control_repo_description.get()
                        on:input=move |ev| set_control_repo_description.set(event_value(&ev))
                    ></textarea>
                </div>
                <div class="gt-inline">
                    <input
                        class="gt-checkbox"
                        type="checkbox"
                        checked=move || control_repo_private.get()
                        on:change=move |ev| set_control_repo_private.set(event_checked(&ev))
                    />
                    <span class="gt-meta">{t!("app.test.control.repo.private")}</span>
                </div>
                <div class="gt-actions">
                    <button
                        class="gt-button gt-button-secondary"
                        disabled=move || {
                            busy.get()
                                || control_repo_name.get().trim().is_empty()
                                || control_token.get().trim().is_empty()
                        }
                        on:click=create_repo_action
                    >
                        {t!("app.test.control.repo.create")}
                    </button>
                </div>
            </div>
            {move || match control_repo_pubkey.get() {
                Some(pubkey) => view! {
                    <div class="gt-status">
                        <p class="gt-meta">{format!("{} {}", t!("app.test.control.repo.pubkey"), pubkey)}</p>
                    </div>
                }
                .into_any(),
                None => ().into_any(),
            }}
            {move || match control_repo_status.get() {
                Some(repo) => view! {
                    <div class="gt-status">
                        <p class="gt-meta">{format!("{} {}/{}", t!("app.test.control.repo.full_name"), repo.owner, repo.name)}</p>
                        {match repo.html_url.clone() {
                            Some(url) => view! {
                                <p class="gt-meta">{format!("{} {}", t!("app.test.control.repo.url"), url)}</p>
                            }
                            .into_any(),
                            None => ().into_any(),
                        }}
                    </div>
                }
                .into_any(),
                None => ().into_any(),
            }}

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
        .or_else(auth_url_from_meta)
        .unwrap_or_default()
}

fn resolve_app_url() -> String {
    app_url_from_context()
        .or_else(app_url_from_dom)
        .or_else(app_url_from_meta)
        .or_else(app_url_from_location)
        .unwrap_or_default()
}

fn resolve_control_url() -> String {
    control_url_from_context()
        .or_else(control_url_from_dom)
        .or_else(control_url_from_meta)
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
fn auth_url_from_meta() -> Option<String> {
    meta_content("gittree-auth-url")
}

#[cfg(not(feature = "ssr"))]
fn app_url_from_meta() -> Option<String> {
    meta_content("gittree-app-url")
}

#[cfg(not(feature = "ssr"))]
fn control_url_from_meta() -> Option<String> {
    meta_content("gittree-control-url")
}

#[cfg(not(feature = "ssr"))]
fn meta_content(name: &str) -> Option<String> {
    use leptos::prelude::window;

    let document = window().document()?;
    let selector = format!("meta[name=\"{name}\"]");
    let element = document.query_selector(&selector).ok()??;
    element.get_attribute("content")
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
fn auth_url_from_meta() -> Option<String> {
    None
}

#[cfg(feature = "ssr")]
fn app_url_from_meta() -> Option<String> {
    None
}

#[cfg(feature = "ssr")]
fn control_url_from_meta() -> Option<String> {
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

fn profile_href(base_path: &str) -> String {
    let base = base_path.trim_end_matches('/');
    if base.is_empty() || base == "/" {
        "/profile".to_string()
    } else {
        format!("{base}/profile")
    }
}

fn account_href(base_path: &str) -> String {
    let base = base_path.trim_end_matches('/');
    if base.is_empty() || base == "/" {
        "/account".to_string()
    } else {
        format!("{base}/account")
    }
}

fn public_profile_href(base_path: &str, npub: &str) -> String {
    let base = base_path.trim_end_matches('/');
    if base.is_empty() || base == "/" {
        format!("/u/{npub}")
    } else {
        format!("{base}/u/{npub}")
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

fn repo_list_by_owner_endpoint(app_url: &str, npub: &str) -> String {
    let trimmed = app_url.trim();
    if trimmed.is_empty() {
        format!("/api/users/{npub}/repos")
    } else {
        format!("{}/api/users/{npub}/repos", trimmed.trim_end_matches('/'))
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

fn auth_source_label(source: AuthSource) -> &'static str {
    match source {
        AuthSource::Nip07 => "nip-07",
        AuthSource::Local => "local",
    }
}

async fn session_sign_nip98(
    session: &AuthSession,
    method: &str,
    url: &str,
    payload_sha256: Option<&str>,
    now: i64,
) -> Result<Nip98Event, AuthError> {
    match session.source {
        AuthSource::Nip07 => {
            let pubkey = nip07_pubkey().await?;
            if pubkey != session.pubkey {
                return Err(AuthError::Js("nip-07 pubkey mismatch".to_string()));
            }
            nip07_sign_nip98(pubkey, method, url, payload_sha256, now).await
        }
        AuthSource::Local => local_key_event(method, url, payload_sha256, now),
    }
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

fn event_value(event: &leptos::ev::Event) -> String {
    event
        .target()
        .and_then(|target| Reflect::get(&target, &JsValue::from_str("value")).ok())
        .and_then(|value| value.as_string())
        .unwrap_or_default()
}

fn event_checked(event: &leptos::ev::Event) -> bool {
    event
        .target()
        .and_then(|target| Reflect::get(&target, &JsValue::from_str("checked")).ok())
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::{
        account_href, health_endpoint, public_profile_href, repo_list_by_owner_endpoint,
        repo_list_endpoint, signup_href, test_href,
    };

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
    fn repo_list_by_owner_endpoint_trims_trailing_slash() {
        assert_eq!(
            repo_list_by_owner_endpoint("http://localhost:8090/", "npub1"),
            "http://localhost:8090/api/users/npub1/repos"
        );
        assert_eq!(
            repo_list_by_owner_endpoint("", "npub1"),
            "/api/users/npub1/repos"
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

    #[test]
    fn account_href_joins_base_path() {
        assert_eq!(account_href("/ui"), "/ui/account");
        assert_eq!(account_href("/"), "/account");
    }

    #[test]
    fn public_profile_href_joins_base_path() {
        assert_eq!(public_profile_href("/ui", "npub1"), "/ui/u/npub1");
        assert_eq!(public_profile_href("/", "npub1"), "/u/npub1");
    }
}
