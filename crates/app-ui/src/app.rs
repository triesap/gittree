#![forbid(unsafe_code)]

use leptos::prelude::*;
use leptos_router::components::{Route, Router, Routes};
use leptos_router::path;

use crate::i18n::app_i18n_init;
use crate::t;

#[component]
pub fn GittreeApp() -> impl IntoView {
    provide_context(app_i18n_init());
    view! {
        <Router>
            <main class="gt-app">
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
    view! {
        <section class="gt-panel">
            <h1 class="gt-title">{t!("app.title")}</h1>
            <p class="gt-tagline">{t!("app.tagline")}</p>
            <p class="gt-meta">{t!("app.repo.loading")}</p>
        </section>
    }
}

#[component]
fn RepoDetailPage() -> impl IntoView {
    view! {
        <section class="gt-panel">
            <h1 class="gt-title">{t!("app.title")}</h1>
            <p class="gt-tagline">{t!("app.tagline")}</p>
            <p class="gt-meta">{t!("app.repo.loading")}</p>
        </section>
    }
}

#[component]
fn NotFoundPage() -> impl IntoView {
    view! {
        <section class="gt-panel">
            <h1 class="gt-title">{t!("app.title")}</h1>
            <p class="gt-tagline">{t!("app.tagline")}</p>
            <p class="gt-meta">"not found"</p>
        </section>
    }
}
