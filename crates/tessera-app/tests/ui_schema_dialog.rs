// SPDX-License-Identifier: GPL-3.0-or-later

//! UI tests of the create and clone dialog: form and JSON in sync, validation, the request
//! body, server errors, clone and the read-only unlock.

// Helpers outside #[test] functions unwrap too; a panic is the test failure we want.
#![allow(clippy::unwrap_used)]

mod common;

use std::sync::{Arc, Mutex};

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AppContext as _, Entity, TestAppContext, WindowHandle, base::Root};
use serde_json::{Value, json};
use tessera_app::connection::ConnectionState;
use tessera_app::state::{AppState, OpenCollection};
use tessera_app::views::schema_dialog::SchemaDialog;
use tessera_client::{Collection, Field};
use tessera_core::profile::Profile;
use tessera_core::schema::SchemaDraft;
use uuid::Uuid;

use common::*;

const SUBMIT: &str = key("cmd-enter", "ctrl-enter");
const SHOW_JSON: &str = key("cmd-2", "ctrl-2");
const SHOW_FORM: &str = key("cmd-1", "ctrl-1");
const NEW_COLLECTION: &str = key("cmd-shift-n", "ctrl-shift-n");
const CLONE: &str = key("cmd-shift-d", "ctrl-shift-d");

struct Fixture {
    handle: WindowHandle<Root>,
    state: Entity<AppState>,
    conn: Entity<ConnectionState>,
    profile: Profile,
    backend: Arc<FakeData>,
    opened: Arc<Mutex<Vec<(Uuid, String)>>>,
    _rt: tokio::runtime::Runtime,
}

impl Fixture {
    /// Connects the staging profile, or the read-only prod one, through the sidebar.
    async fn new(cx: &mut TestAppContext, read_only: bool) -> Self {
        fake_connector(cx, |p| Ok(server_for(p)));
        let backend = fake_backend(cx, Some((vec![books()], Vec::new())));
        let (staging, prod, secrets) = two_profiles();
        let profile = if read_only { prod } else { staging };
        let (handle, state, rt) = open_app(cx, vec![profile.clone()], secrets);
        let opened = Arc::new(Mutex::new(Vec::new()));
        let record = opened.clone();
        cx.update(|cx| {
            cx.subscribe(&state, move |_, event: &OpenCollection, _| {
                record.lock().unwrap().push((event.0, event.1.clone()));
            })
            .detach();
        });
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click(entry(&profile), cx);
        })
        .unwrap();
        wait_for_label(cx, handle, "status.text".into(), "Connected").await;
        let conn = cx.update(|cx| state.read(cx).connection(profile.id, cx).unwrap());
        Self {
            handle,
            state,
            conn,
            profile,
            backend,
            opened,
            _rt: rt,
        }
    }

    fn create(&self, cx: &mut TestAppContext) -> Entity<SchemaDialog> {
        let (state, conn) = (self.state.clone(), self.conn.clone());
        let dialog = cx
            .update_window(self.handle.into(), |_, window, cx| {
                SchemaDialog::open_create(state, conn, window, cx)
            })
            .unwrap();
        self.settle(cx);
        dialog
    }

    fn clone_of(&self, cx: &mut TestAppContext, source: Collection) -> Entity<SchemaDialog> {
        let (state, conn) = (self.state.clone(), self.conn.clone());
        let dialog = cx
            .update_window(self.handle.into(), |_, window, cx| {
                SchemaDialog::open_clone(state, conn, source, window, cx)
            })
            .unwrap();
        self.settle(cx);
        dialog
    }

    /// Runs `step`, lets its effects run and draws a frame.
    fn act(
        &self,
        cx: &mut TestAppContext,
        step: impl FnOnce(&mut gpui_kit::Window, &mut gpui_kit::App),
    ) {
        cx.update_window(self.handle.into(), |_, window, cx| {
            window.render_frame(cx);
            step(window, cx);
        })
        .unwrap();
        self.settle(cx);
    }

    fn settle(&self, cx: &mut TestAppContext) {
        cx.run_until_parked();
        cx.update_window(self.handle.into(), |_, window, cx| window.render_frame(cx))
            .unwrap();
    }

    /// Replaces the JSON text, as typing in the JSON tab does.
    fn set_json(&self, cx: &mut TestAppContext, dialog: &Entity<SchemaDialog>, text: &str) {
        let editor = cx.update(|cx| dialog.read(cx).json_editor().clone());
        self.act(cx, |window, cx| {
            editor.update(cx, |e, cx| e.set_text(text, window, cx));
        });
    }

    fn json(&self, cx: &mut TestAppContext, dialog: &Entity<SchemaDialog>) -> Value {
        let text = cx.update(|cx| dialog.read(cx).json_text(cx));
        serde_json::from_str(&text).unwrap()
    }

    fn query<T>(&self, cx: &mut TestAppContext, f: impl FnOnce(&mut gpui_kit::Window) -> T) -> T {
        cx.update_window(self.handle.into(), |_, window, cx| {
            window.render_frame(cx);
            f(window)
        })
        .unwrap()
    }

    fn creates(&self) -> Vec<Value> {
        self.backend.creates.lock().unwrap().clone()
    }

    fn submit_disabled(&self, cx: &mut TestAppContext) -> bool {
        self.query(cx, |w| w.find("schema.submit").disabled() == Some(true))
    }
}

fn books() -> Collection {
    Collection {
        name: "books".into(),
        num_documents: 3,
        fields: vec![
            Field {
                name: "title".into(),
                r#type: "string".into(),
                attributes: json!({"facet": false, "index": true, "optional": false})
                    .as_object()
                    .cloned()
                    .unwrap(),
            },
            Field {
                name: "embedding".into(),
                r#type: "float[]".into(),
                attributes: json!({"embed": {"from": ["title"], "model_config": {
                    "model_name": "openai/text-embedding-3-small", "api_key": "sk-secret-1"}}})
                .as_object()
                .cloned()
                .unwrap(),
            },
        ],
        metadata: json!({"value": "x"}).as_object().cloned(),
        ..Default::default()
    }
}

#[gpui_kit::test]
async fn form_edit_updates_json(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, false).await;
    let dialog = f.create(cx);
    f.act(cx, |window, cx| {
        fill(window, "schema.name", "books", cx);
        fill(window, "schema.field.0.name", "title", cx);
    });
    f.act(cx, |window, cx| window.click("schema.field.0.facet", cx));
    assert_eq!(
        f.json(cx, &dialog),
        json!({"name": "books", "fields": [{"name": "title", "type": "string", "facet": true}]})
    );
    // Add field appends a row whose name the JSON gets as well.
    f.act(cx, |window, cx| window.click("schema.add_field", cx));
    f.act(cx, |window, cx| {
        fill(window, "schema.field.1.name", "year", cx)
    });
    assert_eq!(
        f.json(cx, &dialog)["fields"][1],
        json!({"name": "year", "type": "string"})
    );
    // The form's own rewrite does not refill the form: the input keeps the text as typed.
    f.act(cx, |window, cx| {
        fill(window, "schema.token_separators", "- /", cx)
    });
    assert_eq!(f.json(cx, &dialog)["token_separators"], json!(["-", "/"]));
    f.query(cx, |w| {
        assert_eq!(w.find("schema.token_separators").value(), Some("- /"))
    });
}

#[gpui_kit::test]
async fn json_edit_updates_form(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, false).await;
    let dialog = f.create(cx);
    f.set_json(
        cx,
        &dialog,
        r#"{"name": "cds", "fields": [{"name": "artist", "type": "string", "facet": true},
            {"name": "year", "type": "int32"}], "default_sorting_field": "year"}"#,
    );
    f.query(cx, |w| {
        assert_eq!(w.find("schema.name").value(), Some("cds"));
        assert_eq!(w.find("schema.field.0.name").value(), Some("artist"));
        assert_eq!(w.find("schema.field.1.name").value(), Some("year"));
        assert_eq!(w.find("schema.field.0.facet").checked(), Some(true));
        let ty = w.within("schema.field.1.type").find("field_type");
        assert!(format!("{ty:?}").contains("int32"), "{ty:?}");
    });
    assert!(!f.submit_disabled(cx));
}

#[gpui_kit::test]
async fn invalid_json_locks_form_and_disables_create(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, false).await;
    let dialog = f.create(cx);
    f.set_json(cx, &dialog, "{\n  \"name\": \"b\",\n}");
    f.query(cx, |w| {
        let text = w
            .find("schema.json_locked")
            .label()
            .unwrap_or_default()
            .to_string();
        assert!(text.contains("line 3"), "{text}");
        // The form is read-only: its controls are gone until the JSON parses.
        assert!(w.try_find("schema.name").is_none());
    });
    assert!(f.submit_disabled(cx));
    f.act(cx, |window, cx| window.click("schema.submit", cx));
    assert!(f.creates().is_empty());
    // The button jumps to the error in the JSON tab.
    f.act(cx, |window, cx| window.click("schema.json_locked", cx));
    f.query(cx, |w| assert!(w.try_find("json.error").is_some()));
    // An empty text is a parse error as well.
    f.set_json(cx, &dialog, "");
    assert!(f.submit_disabled(cx));
    f.set_json(
        cx,
        &dialog,
        r#"{"name": "b", "fields": [{"name": "t", "type": "string"}]}"#,
    );
    assert!(!f.submit_disabled(cx));
    f.act(cx, |window, cx| window.press(SHOW_FORM, cx));
    f.query(cx, |w| assert_eq!(w.find("schema.name").value(), Some("b")));
}

#[gpui_kit::test]
async fn create_sends_the_json_tab_body(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, false).await;
    let dialog = f.create(cx);
    let text = r#"{"name": "books", "fields": [{"name": "title", "type": "string",
        "facet": false, "x_custom": 1}], "zzz": true}"#;
    f.set_json(cx, &dialog, text);
    // ⌘↩ in the JSON editor creates instead of breaking the line.
    f.act(cx, |window, cx| window.press(SHOW_JSON, cx));
    f.act(cx, |window, cx| window.press(SUBMIT, cx));
    let expected = SchemaDraft::from_json(text).unwrap().to_json();
    assert_eq!(f.creates(), [expected]);
    assert_eq!(cx.update(|cx| dialog.read(cx).json_text(cx)), text);
}

#[gpui_kit::test]
async fn validation_error_disables_create(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, false).await;
    let dialog = f.create(cx);
    f.set_json(
        cx,
        &dialog,
        r#"{"name": "b", "fields": [{"name": "t", "type": "string"}, {"name": "t", "type": "int32"}]}"#,
    );
    assert!(f.submit_disabled(cx));
    f.query(cx, |w| {
        let summary = w
            .find("schema.issues")
            .label()
            .unwrap_or_default()
            .to_string();
        assert!(
            summary.contains("Another field already uses this name."),
            "{summary}"
        );
        assert!(w.try_find("schema.field.1.issues").is_some());
    });
    f.act(cx, |window, cx| window.press(SUBMIT, cx));
    assert!(f.creates().is_empty());
}

#[gpui_kit::test]
async fn conflict_keeps_draft_and_shows_message(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, false).await;
    let message = "A collection with name `books` already exists.";
    *f.backend.create_error.lock().unwrap() = Some((409, message.into()));
    let dialog = f.create(cx);
    let text = r#"{"name": "books", "fields": [{"name": "title", "type": "string"}]}"#;
    f.set_json(cx, &dialog, text);
    f.act(cx, |window, cx| window.click("schema.submit", cx));
    wait_for_element(cx, f.handle, "schema.server_error".into(), true).await;
    f.query(cx, |w| {
        let shown = w
            .find("schema.server_error")
            .label()
            .unwrap_or_default()
            .to_string();
        assert!(shown.contains(message), "{shown}");
        assert!(
            w.try_find("schema.submit").is_some(),
            "the dialog stays open"
        );
    });
    assert_eq!(cx.update(|cx| dialog.read(cx).json_text(cx)), text);
    assert_eq!(f.creates().len(), 1);
    assert!(f.opened.lock().unwrap().is_empty());
}

#[gpui_kit::test]
async fn clone_prefills_with_copy_name_and_blank_secrets(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, false).await;
    let dialog = f.clone_of(cx, books());
    let json = f.json(cx, &dialog);
    assert_eq!(json["name"], "books_copy");
    assert_eq!(json["fields"][1]["embed"]["model_config"]["api_key"], "");
    assert!(!json.to_string().contains("sk-secret-1"));
    f.query(cx, |w| {
        assert_eq!(w.find("schema.name").value(), Some("books_copy"));
        assert_eq!(
            w.find("schema.field.1.secret").label(),
            Some("Enter the key for the embedding model again")
        );
        assert!(w.try_find("schema.field.0.secret").is_none());
    });
    // The name starts selected: typing replaces it.
    f.act(cx, |window, cx| window.input("novels", cx));
    assert_eq!(f.json(cx, &dialog)["name"], "novels");
}

#[gpui_kit::test]
async fn create_on_read_only_asks_for_unlock_first(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, true).await;
    let dialog = f.create(cx);
    f.set_json(
        cx,
        &dialog,
        r#"{"name": "b", "fields": [{"name": "t", "type": "string"}]}"#,
    );
    f.act(cx, |window, cx| window.click("schema.submit", cx));
    wait_for_element(cx, f.handle, "danger.input".into(), true).await;
    assert!(f.creates().is_empty());
    let name = f.profile.name.clone();
    f.act(cx, |window, cx| {
        window.click("danger.input".to_string(), cx);
        window.input(&name, cx);
    });
    f.act(cx, |window, cx| window.click("danger.ok".to_string(), cx));
    wait_for_element(cx, f.handle, "danger.input".into(), false).await;
    f.settle(cx);
    assert_eq!(f.creates().len(), 1);
}

#[gpui_kit::test]
async fn success_reloads_list_and_opens_tab(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, false).await;
    let dialog = f.create(cx);
    let lists = *f.backend.lists.lock().unwrap();
    f.set_json(
        cx,
        &dialog,
        r#"{"name": "cds", "fields": [{"name": "t", "type": "string"}]}"#,
    );
    f.act(cx, |window, cx| window.click("schema.submit", cx));
    wait_for_element(cx, f.handle, "schema.submit".into(), false).await;
    assert!(*f.backend.lists.lock().unwrap() > lists, "the list reloads");
    assert_eq!(
        *f.opened.lock().unwrap(),
        [(f.profile.id, "cds".to_string())]
    );
    let cache = cx.update(|cx| f.conn.read(cx).collection_cache.collections.clone());
    assert!(cache.unwrap().iter().any(|c| c.name == "cds"));
}

#[gpui_kit::test]
async fn shortcuts_open_switch_and_add(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, false).await;
    f.act(cx, |window, cx| {
        window.click(format!("sidebar.{}.collections", f.profile.id), cx)
    });
    wait_for_element(cx, f.handle, "collections.row.books".into(), true).await;
    // New collection from the collections tab; focus starts in the name.
    f.act(cx, |window, cx| {
        window.click("collections.search", cx);
        window.press(NEW_COLLECTION, cx);
    });
    f.query(cx, |w| {
        assert_eq!(w.find("schema.name").focused(), Some(true))
    });
    // ⌘↩ in the last row adds a field; ⌘2 and ⌘1 switch tabs.
    f.act(cx, |window, cx| {
        window.click("schema.field.0.name", cx);
        window.press(SUBMIT, cx);
    });
    f.query(cx, |w| assert!(w.try_find("schema.field.1.name").is_some()));
    assert!(f.creates().is_empty());
    f.act(cx, |window, cx| window.press(SHOW_JSON, cx));
    f.query(cx, |w| assert!(w.try_find("schema.field.0.name").is_none()));
    f.act(cx, |window, cx| window.press(SHOW_FORM, cx));
    f.query(cx, |w| assert!(w.try_find("schema.field.1.name").is_some()));
    f.act(cx, |window, cx| window.press("escape", cx));
    wait_for_element(cx, f.handle, "schema.submit".into(), false).await;
    // Clone with ⌘⇧D on the selected row.
    f.act(cx, |window, cx| {
        window.click("collections.row.books", cx);
        window.press(CLONE, cx);
    });
    f.query(cx, |w| {
        assert_eq!(w.find("schema.name").value(), Some("books_copy"))
    });
    f.act(cx, |window, cx| window.press("escape", cx));
    wait_for_element(cx, f.handle, "schema.submit".into(), false).await;
    // And with the schema view's button.
    let id = f.profile.id;
    cx.update(|cx| {
        f.state
            .update(cx, |s, cx| s.open_collection(id, "books".into(), cx))
    });
    wait_for_element(cx, f.handle, "schema.clone".into(), true).await;
    f.act(cx, |window, cx| window.click("schema.clone", cx));
    f.query(cx, |w| {
        assert!(w.try_find("schema.submit").is_some());
        assert_eq!(w.find("schema.field.0.name").value(), Some("title"));
    });
}

#[gpui_kit::test]
async fn schema_view_clone_sends_unmasked_values(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, false).await;
    let id = f.profile.id;
    cx.update(|cx| {
        f.state
            .update(cx, |s, cx| s.open_collection(id, "books".into(), cx))
    });
    wait_for_element(cx, f.handle, "schema.clone".into(), true).await;
    f.act(cx, |window, cx| window.click("schema.clone", cx));
    f.act(cx, |window, cx| window.click("schema.submit", cx));
    let sent = f.creates();
    // The view masks `value`; the clone must not send the mask.
    assert_eq!(sent[0]["metadata"], json!({"value": "x"}));
    assert_eq!(sent[0]["fields"][1]["embed"]["model_config"]["api_key"], "");
}

#[gpui_kit::test]
async fn create_without_connection_says_so(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, false).await;
    let dialog = f.create(cx);
    f.set_json(
        cx,
        &dialog,
        r#"{"name": "b", "fields": [{"name": "t", "type": "string"}]}"#,
    );
    cx.update(|cx| f.conn.update(cx, |c, _| c.connected = None));
    f.act(cx, |window, cx| window.click("schema.submit", cx));
    f.query(cx, |w| {
        assert_eq!(w.find("schema.server_error").label(), Some("Not connected"))
    });
    assert!(f.creates().is_empty());
}
