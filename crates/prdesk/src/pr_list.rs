//! The main window content for M0: the viewer's PR queue in three tabs.

use crate::config::Config;
use chrono::Utc;
use github_client::{GithubClient, QueueTab, TransportError};
use github_types::{CheckState, PrSummary, ReviewDecision};
use gpui::{
    App, Context, FocusHandle, Focusable, Hsla, SharedString, Window, div, prelude::*, rgb,
};
use gpui_component::{
    ActiveTheme as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    tag::Tag,
};
use std::collections::HashMap;
use std::time::Duration;

pub enum LoadState {
    Loading,
    Loaded(Vec<PrSummary>),
    Failed(String),
}

/// Auth preflight status for the configured account.
enum AuthState {
    Checking,
    Ok { viewer: String },
    Failed(String),
}

pub struct PrListView {
    client: GithubClient,
    config: Config,
    auth: AuthState,
    active_tab: QueueTab,
    tabs: HashMap<QueueTab, LoadState>,
    /// Invalidates in-flight fetches when a newer refresh starts.
    generation: u64,
    poll_started: bool,
    pub focus_handle: FocusHandle,
}

impl PrListView {
    pub fn new(config: Config, cx: &mut Context<Self>) -> Self {
        let client = match &config.gh_user {
            Some(user) => GithubClient::gh_cli_as(user.clone()),
            None => GithubClient::gh_cli(),
        };
        let mut this = Self {
            client,
            config,
            auth: AuthState::Checking,
            active_tab: QueueTab::ReviewRequested,
            tabs: HashMap::new(),
            generation: 0,
            poll_started: false,
            focus_handle: cx.focus_handle(),
        };
        this.preflight(cx);
        this
    }

    /// Verifies auth (by fetching the viewer login), then triggers the first
    /// refresh and starts polling.
    fn preflight(&mut self, cx: &mut Context<Self>) {
        self.auth = AuthState::Checking;
        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            let viewer = cx.background_spawn(async move { client.viewer_login() }).await;
            this.update(cx, |app, cx| {
                match viewer {
                    Ok(viewer) => {
                        app.auth = AuthState::Ok { viewer };
                        app.refresh(cx);
                        app.start_polling(cx);
                    }
                    Err(err) => {
                        app.auth = AuthState::Failed(auth_help(&err, &app.config));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.auth, AuthState::Ok { .. }) {
            return;
        }
        self.generation += 1;
        let generation = self.generation;
        for tab in QueueTab::ALL {
            // Keep stale rows visible while refreshing; only show a spinner
            // state when we have nothing yet.
            self.tabs.entry(tab).or_insert(LoadState::Loading);
            let client = self.client.clone();
            let query = tab.search_query(&self.config.scope);
            cx.spawn(async move |this, cx| {
                let fetched = cx
                    .background_spawn(async move { client.search_prs(&query, 30) })
                    .await;
                this.update(cx, |app, cx| {
                    if app.generation != generation {
                        return;
                    }
                    let state = match fetched {
                        Ok(prs) => LoadState::Loaded(prs),
                        Err(err) => LoadState::Failed(format!("{err}")),
                    };
                    app.tabs.insert(tab, state);
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
        cx.notify();
    }

    fn start_polling(&mut self, cx: &mut Context<Self>) {
        if self.poll_started {
            return;
        }
        self.poll_started = true;
        let interval = Duration::from_secs(self.config.poll_seconds.max(15));
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(interval).await;
                if this.update(cx, |app, cx| app.refresh(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .gap_1()
            .px_3()
            .py_2()
            .child(
                div().flex().items_center().gap_1().flex_1().children(
                    QueueTab::ALL.map(|tab| {
                        let count = match self.tabs.get(&tab) {
                            Some(LoadState::Loaded(prs)) => Some(prs.len()),
                            _ => None,
                        };
                        let label = match count {
                            Some(n) => format!("{} ({n})", tab.label()),
                            None => tab.label().to_string(),
                        };
                        let button = Button::new(SharedString::from(format!("tab-{:?}", tab)))
                            .label(label)
                            .small()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.active_tab = tab;
                                cx.notify();
                            }));
                        if tab == self.active_tab {
                            button.primary()
                        } else {
                            button.ghost()
                        }
                    }),
                ),
            )
            .child(
                Button::new("refresh")
                    .label("Refresh")
                    .ghost()
                    .small()
                    .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
            )
    }

    fn render_row(&self, ix: usize, pr: &PrSummary, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let url = pr.url.clone();
        let subtitle = format!(
            "{} {} · {} · {}",
            pr.repo.slug(),
            pr.number,
            pr.author.login,
            relative_time(pr.updated_at),
        );
        let mut badges = div().flex().items_center().gap_1().flex_shrink_0();
        if pr.is_draft {
            badges = badges.child(badge("Draft", rgb(0x8a8f98).into()));
        }
        if let Some(decision) = pr.review_decision {
            let (label, color) = match decision {
                ReviewDecision::Approved => ("Approved", rgb(0x3fb950)),
                ReviewDecision::ChangesRequested => ("Changes requested", rgb(0xf85149)),
                ReviewDecision::ReviewRequired => ("Review required", rgb(0xd29922)),
            };
            badges = badges.child(badge(label, color.into()));
        }
        if let Some(checks) = pr.checks {
            let (label, color) = match checks {
                CheckState::Success => ("CI ✓", rgb(0x3fb950)),
                CheckState::Failure | CheckState::Error => ("CI ✗", rgb(0xf85149)),
                CheckState::Pending => ("CI …", rgb(0xd29922)),
            };
            badges = badges.child(badge(label, color.into()));
        }

        div()
            .id(ix)
            .flex()
            .items_center()
            .gap_3()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .hover(|s| s.bg(cx.theme().accent))
            .cursor_pointer()
            // M1 replaces this with opening an in-app review session.
            .on_click(move |_, _, cx| cx.open_url(&url))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .truncate()
                            .child(SharedString::from(pr.title.clone())),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .truncate()
                            .child(SharedString::from(subtitle)),
                    ),
            )
            .child(badges)
    }

    fn render_body(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        match &self.auth {
            AuthState::Checking => centered_note("Checking GitHub authentication…", cx),
            AuthState::Failed(help) => div()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .flex_1()
                .gap_3()
                .p_8()
                .child(
                    div()
                        .text_color(cx.theme().muted_foreground)
                        .whitespace_normal()
                        .child(SharedString::from(help.clone())),
                )
                .child(
                    Button::new("retry-auth")
                        .label("Retry")
                        .primary()
                        .on_click(cx.listener(|this, _, _, cx| this.preflight(cx))),
                )
                .into_any_element(),
            AuthState::Ok { .. } => match self.tabs.get(&self.active_tab) {
                None | Some(LoadState::Loading) => centered_note("Loading pull requests…", cx),
                Some(LoadState::Failed(err)) => {
                    centered_note(&format!("Failed to load: {err}"), cx)
                }
                Some(LoadState::Loaded(prs)) if prs.is_empty() => {
                    centered_note("No open pull requests here. Enjoy the silence.", cx)
                }
                Some(LoadState::Loaded(prs)) => div()
                    .id("pr-rows")
                    .flex_1()
                    .overflow_y_scroll()
                    .children(
                        prs.iter()
                            .enumerate()
                            .map(|(ix, pr)| self.render_row(ix, pr, cx))
                            .collect::<Vec<_>>(),
                    )
                    .into_any_element(),
            },
        }
    }
}

impl Focusable for PrListView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for PrListView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let account = match &self.auth {
            AuthState::Ok { viewer } => format!("@{viewer}"),
            _ => self
                .config
                .gh_user
                .as_deref()
                .map(|u| format!("@{u}"))
                .unwrap_or_default(),
        };
        div()
            .track_focus(&self.focus_handle)
            .flex()
            .flex_col()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_3()
                    .pt_2()
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::BOLD)
                            .child("Pull Requests"),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(SharedString::from(account)),
                    ),
            )
            .child(self.render_tabs(cx))
            .child(self.render_body(cx))
    }
}

fn badge(label: &'static str, color: Hsla) -> impl IntoElement {
    Tag::custom(color.opacity(0.15), color, color.opacity(0.4))
        .small()
        .child(SharedString::from(label))
}

fn centered_note(text: &str, cx: &App) -> gpui::AnyElement {
    div()
        .flex()
        .items_center()
        .justify_center()
        .flex_1()
        .text_color(cx.theme().muted_foreground)
        .child(SharedString::from(text.to_string()))
        .into_any_element()
}

fn auth_help(err: &TransportError, config: &Config) -> String {
    match err {
        TransportError::AccountMissing(account) => format!(
            "GitHub account @{account} isn't logged in to the gh CLI yet.\n\nRun this in a terminal, then hit Retry:\n\n    gh auth login --hostname github.com --web\n\n(gh supports multiple accounts; your current active account is untouched.)"
        ),
        TransportError::GhMissing => {
            "The `gh` CLI isn't installed. Install it (brew install gh), run `gh auth login`, then hit Retry.".to_string()
        }
        TransportError::NotAuthenticated => {
            "gh isn't authenticated. Run `gh auth login`, then hit Retry.".to_string()
        }
        other => match &config.gh_user {
            Some(user) => format!("Couldn't reach GitHub as @{user}: {other}"),
            None => format!("Couldn't reach GitHub: {other}"),
        },
    }
}

fn relative_time(t: chrono::DateTime<Utc>) -> String {
    let delta = Utc::now().signed_duration_since(t);
    let (n, unit) = if delta.num_days() >= 1 {
        (delta.num_days(), "d")
    } else if delta.num_hours() >= 1 {
        (delta.num_hours(), "h")
    } else {
        (delta.num_minutes().max(0), "m")
    };
    format!("{n}{unit} ago")
}
