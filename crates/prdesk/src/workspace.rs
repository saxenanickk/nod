//! Root view: switches between the PR list and an open PR session.

use crate::config::Config;
use crate::pr_list::{PrListEvent, PrListView};
use crate::pr_session::{PrSessionView, SessionEvent};
use github_client::GithubClient;
use github_types::PrSummary;
use gpui::{
    App, Context, Entity, FocusHandle, Focusable, Subscription, Window, div, prelude::*,
};

pub struct Workspace {
    client: GithubClient,
    pr_list: Entity<PrListView>,
    session: Option<Entity<PrSessionView>>,
    _list_subscription: Subscription,
    _session_subscription: Option<Subscription>,
    pub focus_handle: FocusHandle,
}

impl Workspace {
    pub fn new(config: Config, cx: &mut Context<Self>) -> Self {
        let client = match &config.gh_user {
            Some(user) => GithubClient::gh_cli_as(user.clone()),
            None => GithubClient::gh_cli(),
        };
        let pr_list = cx.new(|cx| PrListView::new(client.clone(), config, cx));
        let list_subscription =
            cx.subscribe(&pr_list, |this, _, event: &PrListEvent, cx| match event {
                PrListEvent::OpenPr(pr) => this.open_session(pr.clone(), cx),
            });
        Self {
            client,
            pr_list,
            session: None,
            _list_subscription: list_subscription,
            _session_subscription: None,
            focus_handle: cx.focus_handle(),
        }
    }

    fn open_session(&mut self, pr: PrSummary, cx: &mut Context<Self>) {
        let client = self.client.clone();
        let session = cx.new(|cx| PrSessionView::new(client, pr, cx));
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
}

impl Focusable for Workspace {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let content: gpui::AnyElement = match &self.session {
            Some(session) => session.clone().into_any_element(),
            None => self.pr_list.clone().into_any_element(),
        };
        div()
            .track_focus(&self.focus_handle)
            .size_full()
            .child(content)
    }
}
