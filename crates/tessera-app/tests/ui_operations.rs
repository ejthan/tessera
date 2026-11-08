// SPDX-License-Identifier: GPL-3.0-or-later

//! UI tests of the Ops tab.

// Tests unwrap; a panic is the failure we want.
#![allow(clippy::unwrap_used)]

mod common;

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::dock::DockPlacement;
use gpui_kit::test::{TestAppContextExt as _, TestWindowExt as _};
use gpui_kit::{AppContext as _, Entity, Focusable as _, TestAppContext, WindowHandle, base::Root};
use tessera_app::state::{AppState, ResourceKind};
use tessera_app::views::dock::ResourcePanel;
use tessera_app::views::resources::ResourceView;
use tessera_app::views::resources::operations::OperationsView;
use tessera_app::views::root::RootView;
use tessera_client::{Debug, RaftState, SchemaChange};
use tessera_core::profile::{Environment, Node, Profile, Protocol};

use common::*;

struct Fixture {
    handle: WindowHandle<Root>,
    admin: Arc<FakeAdmin>,
    state: Entity<AppState>,
    _rt: tokio::runtime::Runtime,
}

fn node_at(host: &str) -> Node {
    Node {
        protocol: Protocol::Http,
        host: host.into(),
        port: 8108,
        path: String::new(),
    }
}

fn cluster() -> Profile {
    Profile::new("test", vec![node_at("ts1"), node_at("ts2")])
}

fn prod(mut profile: Profile) -> Profile {
    profile.environment = Environment::Prod;
    profile
}

impl Fixture {
    async fn with(cx: &mut TestAppContext, profile: Profile) -> Self {
        fake_backend(cx, Some((vec![], vec![])));
        fake_documents(cx);
        install_search_fake(cx);
        let admin = fake_admin(cx);
        let id = profile.id;
        fake_connector(cx, |profile| Ok(server_for(profile)));
        let secrets = Arc::new(tessera_core::secrets::MemoryStore::default());
        tessera_core::secrets::SecretStore::set(&*secrets, id, &"k".to_string().into()).unwrap();
        let (handle, state, rt) = open_app(cx, vec![profile], secrets);
        cx.update_window(handle.into(), |_, window, cx| window.render_frame(cx))
            .unwrap();
        cx.update(|cx| state.update(cx, |s, cx| s.connect(id, None, cx)));
        cx.wait_for(handle.into(), Duration::from_millis(1000), |window, _| {
            window.try_find("figure.version").is_some()
        })
        .await;
        cx.update(|cx| {
            state.update(cx, |s, cx| {
                s.open_resource(id, ResourceKind::Operations, cx)
            });
        });
        wait_for_element(cx, handle, "ops.compact.run".into(), true).await;
        let f = Self {
            handle,
            admin,
            state,
            _rt: rt,
        };
        f.settle(cx);
        f
    }

    fn settle(&self, cx: &mut TestAppContext) {
        cx.run_until_parked();
        cx.update_window(self.handle.into(), |_, window, cx| window.render_frame(cx))
            .unwrap();
        cx.run_until_parked();
    }

    fn has(&self, cx: &mut TestAppContext, id: &str) -> bool {
        self.settle(cx);
        cx.update_window(self.handle.into(), |_, window, _| {
            window.try_find(id.to_string()).is_some()
        })
        .unwrap()
    }

    fn label(&self, cx: &mut TestAppContext, id: &str) -> String {
        self.settle(cx);
        cx.update_window(self.handle.into(), |_, window, _| {
            let found = window.try_find(id.to_string());
            found.and_then(|e| e.label().map(ToString::to_string))
        })
        .unwrap()
        .unwrap_or_default()
    }

    fn click(&self, cx: &mut TestAppContext, id: &str) {
        self.settle(cx);
        cx.update_window(self.handle.into(), |_, window, cx| {
            window.click(id.to_string(), cx);
        })
        .unwrap();
        self.settle(cx);
    }

    fn fill(&self, cx: &mut TestAppContext, id: &str, text: &str) {
        self.settle(cx);
        cx.update_window(self.handle.into(), |_, window, cx| {
            fill(window, id, text, cx);
        })
        .unwrap();
        self.settle(cx);
    }

    fn view(&self, cx: &mut TestAppContext) -> Entity<OperationsView> {
        let panel = self
            .handle
            .update(cx, |root, _, cx| {
                let view = root.view().clone().downcast::<RootView>().unwrap();
                let area = view.read(cx).dock().read(cx);
                area.layout(DockPlacement::Center)
                    .unwrap()
                    .panels()
                    .filter_map(|id| area.panel(id)?.view().downcast::<ResourcePanel>().ok())
                    .next()
                    .unwrap()
            })
            .unwrap();
        cx.update(|cx| {
            let ResourceView::Operations(view) = panel.read(cx).view() else {
                panic!("not the Ops view");
            };
            view.clone()
        })
    }

    fn pick_node(&self, cx: &mut TestAppContext, index: usize) {
        let view = self.view(cx);
        cx.update(|cx| view.update(cx, |v, cx| v.pick_node_at(index, cx)));
        self.settle(cx);
    }

    fn pick_config(&self, cx: &mut TestAppContext, name: &str) {
        let view = self.view(cx);
        cx.update(|cx| view.update(cx, |v, cx| v.pick_config(name, cx)));
        self.settle(cx);
    }

    /// The node `index` reports `state`.
    fn set_role(&self, cx: &mut TestAppContext, index: usize, state: RaftState) {
        let view = self.view(cx);
        cx.update(|cx| {
            let connection = view.read(cx).connection().clone();
            connection.update(cx, |c, cx| {
                c.nodes[index].debug = Some(Debug {
                    state,
                    version: "30.2".into(),
                });
                cx.notify();
            });
        });
        self.settle(cx);
    }

    fn sent(&self) -> Vec<String> {
        let calls = self.admin.calls.lock().unwrap();
        calls
            .iter()
            .filter(|c| !c.starts_with("schema_changes"))
            .cloned()
            .collect()
    }
}

#[gpui_kit::test]
async fn compact_asks_plainly_and_goes_to_the_picked_node(cx: &mut TestAppContext) {
    let f = Fixture::with(cx, cluster()).await;
    f.pick_node(cx, 1);
    f.click(cx, "ops.compact.run");
    assert!(f.has(cx, "confirm.ok"));
    assert!(f.sent().is_empty(), "nothing goes out before the answer");
    f.click(cx, "confirm.ok");
    assert_eq!(f.sent(), ["compact 1"]);
    let result = f.label(cx, "ops.compact.result");
    assert!(result.contains("ts2:8108"), "{result}");
}

#[gpui_kit::test]
async fn cancel_sends_nothing_and_escape_leaves_focus_sane(cx: &mut TestAppContext) {
    let f = Fixture::with(cx, cluster()).await;
    f.click(cx, "ops.cache.run");
    f.click(cx, "confirm.cancel");
    assert!(!f.has(cx, "confirm.ok"));

    f.click(cx, "ops.cache.run");
    cx.update_window(f.handle.into(), |_, window, cx| window.press("escape", cx))
        .unwrap();
    assert!(!f.has(cx, "confirm.ok"));
    assert!(f.sent().is_empty());
    // A shortcut works again, so focus is inside the window and not on a dead element.
    f.click(cx, "ops.cache.run");
    assert!(f.has(cx, "confirm.ok"));
}

#[gpui_kit::test]
async fn default_focus_is_not_on_a_red_button(cx: &mut TestAppContext) {
    let f = Fixture::with(cx, cluster()).await;
    let focused = |f: &Fixture, cx: &mut TestAppContext, id: &str| {
        f.settle(cx);
        cx.update_window(f.handle.into(), |_, window, _| {
            window.try_find(id.to_string()).and_then(|e| e.focused())
        })
        .unwrap()
    };
    for id in ["ops.snapshot.run", "ops.compact.run", "ops.vote.run"] {
        assert_ne!(focused(&f, cx, id), Some(true), "{id}");
    }
    f.click(cx, "ops.compact.run");
    assert!(f.has(cx, "confirm.ok"));
    assert_ne!(focused(&f, cx, "confirm.ok"), Some(true));
}

#[gpui_kit::test]
async fn snapshot_is_plain_off_prod(cx: &mut TestAppContext) {
    let f = Fixture::with(cx, cluster()).await;
    f.fill(cx, "ops.snapshot.path", "/var/backups/ts");
    f.click(cx, "ops.snapshot.run");
    assert!(!f.has(cx, "danger.input"));
    let body = f.label(cx, "confirm.body");
    assert!(body.contains("/var/backups/ts"), "{body}");
    assert!(body.contains("ts1:8108"), "{body}");
    f.click(cx, "confirm.ok");
    assert_eq!(f.sent(), ["snapshot 0 /var/backups/ts"]);
}

#[gpui_kit::test]
async fn snapshot_and_vote_are_typed_on_prod(cx: &mut TestAppContext) {
    let f = Fixture::with(cx, prod(cluster())).await;
    f.fill(cx, "ops.snapshot.path", "/x");
    f.click(cx, "ops.snapshot.run");
    assert!(f.has(cx, "danger.input"));
    f.click(cx, "danger.ok");
    assert!(f.sent().is_empty(), "the name was not typed");
    f.fill(cx, "danger.input", "test");
    f.click(cx, "danger.ok");
    assert_eq!(f.sent(), ["snapshot 0 /x"]);
    assert!(!f.has(cx, "danger.input"));

    f.set_role(cx, 0, RaftState::Follower);
    f.click(cx, "ops.vote.run");
    assert!(f.has(cx, "danger.input"));
    f.fill(cx, "danger.input", "test");
    f.click(cx, "danger.ok");
    assert_eq!(f.sent(), ["snapshot 0 /x", "vote 0"]);
}

#[gpui_kit::test]
async fn compact_stays_plain_on_prod_and_config_asks_there(cx: &mut TestAppContext) {
    let f = Fixture::with(cx, prod(cluster())).await;
    f.click(cx, "ops.compact.run");
    assert!(f.has(cx, "confirm.ok"));
    assert!(!f.has(cx, "danger.input"));
    f.click(cx, "confirm.cancel");
    f.fill(cx, "ops.config.value", "5");
    f.click(cx, "ops.config.run");
    assert!(f.has(cx, "confirm.ok"));
    f.click(cx, "confirm.ok");
    assert_eq!(f.sent(), ["set_config 0 log-slow-requests-time-ms 5"]);
}

#[gpui_kit::test]
async fn a_failed_typed_snapshot_keeps_its_dialog_and_shows_under_its_row(cx: &mut TestAppContext) {
    let f = Fixture::with(cx, prod(cluster())).await;
    *f.admin.fail.lock().unwrap() = Some((500, "Copy failed.".into()));
    f.fill(cx, "ops.snapshot.path", "/x");
    f.click(cx, "ops.snapshot.run");
    f.fill(cx, "danger.input", "test");
    f.click(cx, "danger.ok");
    assert!(f.label(cx, "danger.error").contains("Copy failed."));
    assert!(f.label(cx, "ops.snapshot.result").contains("Copy failed."));
}

#[gpui_kit::test]
async fn read_only_unlocks_first(cx: &mut TestAppContext) {
    let mut profile = cluster();
    profile.read_only = true;
    let f = Fixture::with(cx, profile).await;
    f.click(cx, "ops.compact.run");
    assert!(f.has(cx, "danger.input"), "the unlock dialog opens");
    assert!(f.sent().is_empty());
    f.fill(cx, "danger.input", "test");
    f.click(cx, "danger.ok");
    assert!(f.has(cx, "confirm.ok"), "the operation still asks");
    f.click(cx, "confirm.ok");
    assert_eq!(f.sent(), ["compact 0"]);
}

#[gpui_kit::test]
async fn a_bad_path_says_why_and_sends_nothing(cx: &mut TestAppContext) {
    let f = Fixture::with(cx, cluster()).await;
    let view = f.view(cx);
    f.fill(cx, "ops.snapshot.path", "relative");
    assert!(f.has(cx, "ops.snapshot.problem"));
    assert!(cx.read(|cx| !view.read(cx).can_snapshot(cx)));
    f.click(cx, "ops.snapshot.run");
    assert!(!f.has(cx, "confirm.ok"));
    f.fill(cx, "ops.snapshot.path", "/fine");
    assert!(!f.has(cx, "ops.snapshot.problem"));
    assert!(cx.read(|cx| view.read(cx).can_snapshot(cx)));
}

#[gpui_kit::test]
async fn config_sends_one_key_at_once_off_prod(cx: &mut TestAppContext) {
    let f = Fixture::with(cx, cluster()).await;
    f.pick_config(cx, "cache-num-entries");
    f.fill(cx, "ops.config.value", "0");
    assert!(f.has(cx, "ops.config.problem"));
    f.click(cx, "ops.config.run");
    assert!(f.sent().is_empty());
    f.fill(cx, "ops.config.value", "500");
    assert!(f.label(cx, "ops.config.note").contains("restarts"));
    f.click(cx, "ops.config.run");
    assert!(!f.has(cx, "confirm.ok"));
    assert_eq!(f.sent(), ["set_config 0 cache-num-entries 500"]);
}

#[gpui_kit::test]
async fn config_offers_a_switch_for_a_bool(cx: &mut TestAppContext) {
    let f = Fixture::with(cx, cluster()).await;
    f.pick_config(cx, "enable-search-logging");
    assert!(!f.has(cx, "ops.config.value"));
    f.click(cx, "ops.config.flag");
    f.click(cx, "ops.config.run");
    assert_eq!(f.sent(), ["set_config 0 enable-search-logging true"]);
}

#[gpui_kit::test]
async fn vote_needs_a_follower(cx: &mut TestAppContext) {
    let f = Fixture::with(cx, Profile::new("one", vec![node()])).await;
    assert!(f.has(cx, "ops.vote.block"));
    f.click(cx, "ops.vote.run");
    assert!(!f.has(cx, "confirm.ok"));

    let f = Fixture::with(cx, cluster()).await;
    f.set_role(cx, 0, RaftState::Leader);
    assert!(f.has(cx, "ops.vote.block"));
    f.pick_node(cx, 1);
    f.set_role(cx, 1, RaftState::Follower);
    assert!(!f.has(cx, "ops.vote.block"));
    f.click(cx, "ops.vote.run");
    f.click(cx, "confirm.ok");
    assert_eq!(f.sent(), ["vote 1"]);
    assert!(f.label(cx, "ops.vote.result").contains("ts2:8108"));
}

#[gpui_kit::test]
async fn a_refused_vote_is_a_result(cx: &mut TestAppContext) {
    let f = Fixture::with(cx, cluster()).await;
    *f.admin.vote_refused.lock().unwrap() = true;
    f.set_role(cx, 0, RaftState::Follower);
    f.click(cx, "ops.vote.run");
    f.click(cx, "confirm.ok");
    assert!(f.label(cx, "ops.vote.result").contains("refused"));
}

#[gpui_kit::test]
async fn a_result_keeps_its_node_after_another_is_picked(cx: &mut TestAppContext) {
    let f = Fixture::with(cx, cluster()).await;
    f.click(cx, "ops.cache.run");
    f.click(cx, "confirm.ok");
    f.pick_node(cx, 1);
    assert!(f.label(cx, "ops.cache.result").contains("ts1:8108"));
}

#[gpui_kit::test]
async fn running_changes_show_and_an_empty_list_says_so(cx: &mut TestAppContext) {
    let f = Fixture::with(cx, cluster()).await;
    assert!(f.has(cx, "ops.schema.empty"));
    *f.admin.schema_changes.lock().unwrap() = vec![SchemaChange {
        collection: "books".into(),
        validated_docs: 7,
        altered_docs: 3,
    }];
    cx.executor().advance_clock(Duration::from_secs(3));
    assert_eq!(f.label(cx, "ops.schema.row.books"), "books 7 3");
    assert!(!f.has(cx, "ops.schema.empty"));
    let calls = f.admin.calls.lock().unwrap();
    assert!(
        calls.iter().any(|c| c == "schema_changes 0 polling"),
        "{calls:?}"
    );
}

#[gpui_kit::test]
async fn the_shortcuts_reach_each_row(cx: &mut TestAppContext) {
    let f = Fixture::with(cx, cluster()).await;
    let press = |f: &Fixture, cx: &mut TestAppContext, chord: &'static str| {
        f.settle(cx);
        let view = f.view(cx);
        cx.update_window(f.handle.into(), |_, window, cx| {
            view.read(cx).focus_handle(cx).focus(window, cx);
            window.press(chord, cx);
        })
        .unwrap();
        f.settle(cx);
    };
    press(&f, cx, key("cmd-2", "ctrl-2"));
    assert!(f.label(cx, "confirm.body").contains("ts1:8108"));
    f.click(cx, "confirm.ok");
    assert_eq!(f.sent(), ["compact 0"]);
    press(&f, cx, key("cmd-3", "ctrl-3"));
    f.click(cx, "confirm.ok");
    assert_eq!(f.sent(), ["compact 0", "clear_cache 0"]);
}

#[gpui_kit::test]
async fn polling_stops_while_another_tab_shows(cx: &mut TestAppContext) {
    let f = Fixture::with(cx, cluster()).await;
    let polls = |f: &Fixture| {
        let calls = f.admin.calls.lock().unwrap();
        calls
            .iter()
            .filter(|c| c.starts_with("schema_changes"))
            .count()
    };
    let tick = |f: &Fixture, cx: &mut TestAppContext| {
        cx.executor().advance_clock(Duration::from_secs(3));
        f.settle(cx);
    };
    tick(&f, cx);
    let before = polls(&f);
    tick(&f, cx);
    assert!(polls(&f) > before, "it polls while it shows");

    let view = f.view(cx);
    cx.update(|cx| {
        let id = view.read(cx).connection().read(cx).profile.id;
        f.state
            .update(cx, |s, cx| s.open_resource(id, ResourceKind::Keys, cx));
    });
    wait_for_element(cx, f.handle, "keys.reload".into(), true).await;
    tick(&f, cx);
    tick(&f, cx);
    let hidden = polls(&f);
    tick(&f, cx);
    tick(&f, cx);
    assert_eq!(polls(&f), hidden);
}
