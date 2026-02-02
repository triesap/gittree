#![forbid(unsafe_code)]

use leptos::prelude::*;
use leptos::prelude::IntoAny;
use leptos_router::components::{Route, Router, Routes};
use leptos_router::hooks::use_params_map;
use leptos_router::path;

use crate::i18n::app_i18n_init;
use crate::server::{list_repositories, repo_detail};
use crate::t;

#[derive(Clone)]
struct AppBasePath(String);

#[component]
pub fn GittreeApp() -> impl IntoView {
    provide_context(app_i18n_init());
    let base_path = resolve_base_path();
    provide_context(AppBasePath(base_path.clone()));

    view! {
        <Router>
            <main
                id="gittree-app"
                class="gt-app"
                data-base-path=base_path
            >
                <Routes fallback=|| view! { <NotFoundPage /> }>
                    <Route path=path!("/") view=RepoListPage />
                    <Route path=path!("/:npub/:identifier") view=RepoDetailPage />
                </Routes>
            </main>
        </Router>
    }
}

#[component]
fn RepoListPage() -> impl IntoView {
    let base_path = app_base_path();
    let repos = Resource::new(|| (), |_| list_repositories());

    view! {
        <section class="gt-panel">
            <h1 class="gt-title">{t!("app.title")}</h1>
            <p class="gt-tagline">{t!("app.tagline")}</p>
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

#[cfg(feature = "ssr")]
fn base_path_from_context() -> Option<String> {
    use crate::AppUiState;

    use_context::<AppUiState>().map(|state| state.base_path)
}

#[cfg(not(feature = "ssr"))]
fn base_path_from_context() -> Option<String> {
    None
}

#[cfg(not(feature = "ssr"))]
fn base_path_from_dom() -> Option<String> {
    use leptos::prelude::window;

    let document = window().document()?;
    let element = document.get_element_by_id("gittree-app")?;
    element.get_attribute("data-base-path")
}

#[cfg(feature = "ssr")]
fn base_path_from_dom() -> Option<String> {
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
