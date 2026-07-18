//! Root view: a PRs/Repos top nav over the PR list and repo picker, plus a
//! full-window PR or local review session when one is open.

use crate::config::Config;
use crate::local_session::{LocalSessionEvent, LocalSessionView};
use crate::pr_list::{PrListEvent, PrListView};
use crate::pr_session::{PrSessionView, SessionEvent};
use crate::repo_picker::{RepoPickerEvent, RepoPickerView};
use github_client::GithubClient;
use github_types::{PrSummary, RepoId};
use gpui::{
    App, Context, Entity, FocusHandle, Focusable, Subscription, Window, div, prelude::*,
};
use gpui_component::{
    ActiveTheme as _, Sizable as _,
    button::{Button, ButtonVariants as _},
};
use std::path::PathBuf;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Prs,
    Repos,
}

pub struct Workspace {
    client: GithubClient,
    config: Config,
    tab: Tab,
    pr_list: Entity<PrListView>,
    repo_picker: Entity<RepoPickerView>,
    session: Option<Entity<PrSessionView>>,
    local: Option<Entity<LocalSessionView>>,
    /// A local repo to open on the next render (opening needs a window, which
    /// event callbacks lack).
    pending_local: Option<PathBuf>,
    _list_subscription: Subscription,
    _picker_subscription: Subscription,
    _session_subscription: Option<Subscription>,
    _local_subscription: Option<Subscription>,
    pub focus_handle: FocusHandle,
}

impl Workspace {
    pub fn new(config: Config, cx: &mut Context<Self>) -> Self {
        let client = match &config.gh_user {
            Some(user) => GithubClient::gh_cli_as(user.clone()),
            None => GithubClient::gh_cli(),
        };
        let pr_list = cx.new(|cx| PrListView::new(client.clone(), config.clone(), cx));
        let repo_picker = cx.new(|cx| RepoPickerView::new(config.clone(), cx));
        let list_subscription =
            cx.subscribe(&pr_list, |this, _, event: &PrListEvent, cx| match event {
                PrListEvent::OpenPr(pr) => this.open_session(pr.clone(), cx),
            });
        let picker_subscription = cx.subscribe(
            &repo_picker,
            |this, _, event: &RepoPickerEvent, cx| match event {
                RepoPickerEvent::OpenLocal(path) => {
                    this.pending_local = Some(path.clone());
                    cx.notify();
                }
                RepoPickerEvent::OpenRepoPrs(repo) => this.open_repo_prs(repo.clone(), cx),
            },
        );
        Self {
            client,
            config,
            tab: Tab::Prs,
            pr_list,
            repo_picker,
            session: None,
            local: None,
            pending_local: None,
            _list_subscription: list_subscription,
            _picker_subscription: picker_subscription,
            _session_subscription: None,
            _local_subscription: None,
            focus_handle: cx.focus_handle(),
        }
    }

    /// Opens a PR session by `owner/repo#number` with a minimal summary
    /// (fetch only needs repo + number; other fields fill in on refresh).
    /// Used by the local→PR bridge and the `PRDESK_DEBUG_PR` hook.
    pub fn open_pr_by_number(&mut self, repo: RepoId, number: u64, cx: &mut Context<Self>) {
        let pr = PrSummary {
            repo,
            number: github_types::PrNumber(number),
            node_id: github_types::NodeId(String::new()),
            title: format!("debug #{number}"),
            author: github_types::Actor { login: "debug".into(), avatar_url: None },
            is_draft: false,
            state: github_types::PrState::Open,
            review_decision: None,
            checks: None,
            head_ref: String::new(),
            head_sha: String::new(),
            updated_at: chrono::Utc::now(),
            url: String::new(),
        };
        self.open_session(pr, cx);
    }

    fn open_session(&mut self, pr: PrSummary, cx: &mut Context<Self>) {
        let client = self.client.clone();
        let config = self.config.clone();
        let session = cx.new(|cx| PrSessionView::new(client, pr, &config, cx));
        self._session_subscription = Some(cx.subscribe(
            &session,
            |this, _, event: &SessionEvent, cx| match event {
                SessionEvent::Close => {
                    this.session = None;
                    this._session_subscription = None;
                    cx.notify();
                }
            },
        ));
        self.session = Some(session);
        cx.notify();
    }

    fn open_repo_prs(&mut self, repo: RepoId, cx: &mut Context<Self>) {
        let scope = format!("repo:{}", repo.slug());
        self.pr_list.update(cx, |list, cx| list.set_scope(Some(scope), cx));
        self.tab = Tab::Prs;
        cx.notify();
    }

    pub fn open_local(&mut self, clone: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let local = cx.new(|cx| LocalSessionView::new(clone, window, cx));
        self._local_subscription = Some(cx.subscribe(
            &local,
            |this, _, event: &LocalSessionEvent, cx| match event {
                LocalSessionEvent::Close => {
                    this.local = None;
                    this._local_subscription = None;
                    cx.notify();
                }
                LocalSessionEvent::OpenPr { repo, number } => {
                    // Leave local review and open the real PR (commenting works there).
                    this.local = None;
                    this._local_subscription = None;
                    this.open_pr_by_number(repo.clone(), *number, cx);
                }
            },
        ));
        window.focus(&local.read(cx).focus_handle);
        self.local = Some(local);
        cx.notify();
    }

    fn render_nav(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let tab_button = |tab: Tab, label: &'static str, active: bool, cx: &mut Context<Self>| {
            let btn = Button::new(label).label(label).small();
            let btn = if active { btn.primary() } else { btn.ghost() };
            btn.on_click(cx.listener(move |this, _, _, cx| {
                this.tab = tab;
                cx.notify();
            }))
        };
        div()
            .flex()
            .items_center()
            .gap_1()
            .px_3()
            .py_1()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(tab_button(Tab::Prs, "PRs", self.tab == Tab::Prs, cx))
            .child(tab_button(Tab::Repos, "Repos", self.tab == Tab::Repos, cx))
    }
}

impl Focusable for Workspace {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Opening a local session needs a window; drain the request queued by
        // the repo-picker event here.
        if let Some(path) = self.pending_local.take() {
            self.open_local(path, window, cx);
        }
        // An open session takes the whole window (it has its own Back button).
        if let Some(local) = &self.local {
            return div()
                .track_focus(&self.focus_handle)
                .size_full()
                .child(local.clone())
                .into_any_element();
        }
        if let Some(session) = &self.session {
            return div()
                .track_focus(&self.focus_handle)
                .size_full()
                .child(session.clone())
                .into_any_element();
        }
        let content: gpui::AnyElement = match self.tab {
            Tab::Prs => self.pr_list.clone().into_any_element(),
            Tab::Repos => self.repo_picker.clone().into_any_element(),
        };
        div()
            .track_focus(&self.focus_handle)
            .flex()
            .flex_col()
            .size_full()
            .child(self.render_nav(cx))
            .child(div().flex_1().min_h_0().child(content))
            .into_any_element()
    }
}
