// SPDX-License-Identifier: GPL-3.0-or-later

//! UI tests of the schema dialog: form and JSON in sync, validation, the request body, server
//! errors, clone, edit with review and progress, and the read-only unlock.

// Helpers outside #[test] functions unwrap too; a panic is the test failure we want.
#![allow(clippy::unwrap_used)]

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AppContext as _, Entity, TestAppContext, WindowHandle, base::Root};
use serde_json::{Value, json};
use tessera_app::connection::ConnectionState;
use tessera_app::state::{AppState, OpenCollection};
use tessera_app::views::schema_dialog::SchemaDialog;
use tessera_client::{Collection, Field, SchemaChange};
use tessera_core::profile::Profile;
use tessera_core::schema::SchemaDraft;
use uuid::Uuid;

use common::*;

const SUBMIT: &str = key("cmd-enter", "ctrl-enter");
const SHOW_JSON: &str = key("cmd-2", "ctrl-2");
const SHOW_FORM: &str = key("cmd-1", "ctrl-1");
const NEW_COLLECTION: &str = key("cmd-shift-n", "ctrl-shift-n");
const CLONE: &str = key("cmd-shift-d", "ctrl-shift-d");
const EDIT_SCHEMA: &str = EDIT;

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

    /// Opens the edit dialog of `collection`, with `collections` as the server's list.
    fn edit_of(
        &self,
        cx: &mut TestAppContext,
        collection: Collection,
        collections: Vec<Collection>,
    ) -> Entity<SchemaDialog> {
        *self.backend.data.lock().unwrap() = Some((collections, Vec::new()));
        cx.update(|cx| self.conn.update(cx, |c, cx| c.invalidate_collections(cx)));
        self.settle(cx);
        let (state, conn) = (self.state.clone(), self.conn.clone());
        let dialog = cx
            .update_window(self.handle.into(), |_, window, cx| {
                SchemaDialog::open_edit(state, conn, collection, window, cx)
            })
            .unwrap();
        self.settle(cx);
        dialog
    }

    /// Changes the JSON tab's content with `change`.
    fn edit_json(
        &self,
        cx: &mut TestAppContext,
        dialog: &Entity<SchemaDialog>,
        change: impl FnOnce(&mut Value),
    ) {
        let mut json = self.json(cx, dialog);
        change(&mut json);
        self.set_json(cx, dialog, &json.to_string());
    }

    /// Edits `library()` the way the review tests expect and opens the review.
    fn review_library_change(&self, cx: &mut TestAppContext) -> Entity<SchemaDialog> {
        let dialog = self.edit_of(cx, library(), vec![library(), reviews()]);
        self.edit_json(cx, &dialog, library_change);
        // The dialog of four fields is taller than the test window; the keys reach it.
        self.act(cx, |window, cx| window.press(SUBMIT, cx));
        dialog
    }

    /// Tabs from Back to Apply and presses Enter there.
    fn apply(&self, cx: &mut TestAppContext) {
        self.act(cx, |window, cx| {
            focus(window, "schema.review.apply", cx);
            window.press("enter", cx);
        });
    }

    fn label_of(&self, cx: &mut TestAppContext, id: &'static str) -> String {
        self.query(cx, |w| label(w, id))
    }

    fn updates(&self) -> Vec<(String, Value)> {
        self.backend.updates.lock().unwrap().clone()
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

/// A collection with 1000 documents, a default sorting field, an embedding with a key and
/// metadata the schema view masks.
fn library() -> Collection {
    serde_json::from_value(json!({
        "name": "books", "num_documents": 1000, "created_at": 1,
        "default_sorting_field": "year",
        "fields": [
            {"name": "title", "type": "string", "facet": false, "index": true, "optional": false},
            {"name": "author", "type": "string", "facet": true, "index": true, "optional": false},
            {"name": "year", "type": "int32", "facet": false, "index": true, "optional": false},
            {"name": "embedding", "type": "float[]", "embed": {"from": ["title"],
                "model_config": {"model_name": "openai/text-embedding-3-small",
                "api_key": "sk-secret-1"}}}
        ],
        "metadata": {"value": "x"}
    }))
    .unwrap()
}

/// A collection whose field references `books.author`.
fn reviews() -> Collection {
    serde_json::from_value(json!({
        "name": "reviews", "num_documents": 5, "created_at": 1,
        "fields": [{"name": "book_author", "type": "string", "reference": "books.author"}]
    }))
    .unwrap()
}

/// Facet on for title, author dropped, pages added.
fn library_change(json: &mut Value) {
    let fields = json["fields"].as_array_mut().unwrap();
    fields[0]["facet"] = json!(true);
    fields.retain(|f| f["name"] != "author");
    fields.push(json!({"name": "pages", "type": "int32"}));
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

#[gpui_kit::test]
async fn edit_locks_name_and_fixed_options(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, false).await;
    let dialog = f.edit_of(cx, books(), vec![books()]);
    let before = f.json(cx, &dialog);
    // The fixed inputs take no focus, and the fixed checkbox no click.
    f.act(cx, |window, cx| {
        for id in [
            "schema.name",
            "schema.token_separators",
            "schema.symbols_to_index",
        ] {
            window.click(id, cx);
            assert_ne!(window.find(id).focused(), Some(true), "{id}");
        }
        window.click("schema.nested", cx);
    });
    f.query(cx, |w| {
        assert_eq!(w.find("schema.name").value(), Some("books"));
        assert_eq!(w.find("schema.token_separators").value(), Some(""));
        assert_eq!(w.find("schema.submit").label(), Some("Review changes…"));
        // The embedding key never shows, and the unchanged field needs none.
        assert!(w.try_find("schema.field.1.secret").is_none());
    });
    assert_eq!(f.json(cx, &dialog), before);
    assert!(!before.to_string().contains("sk-secret-1"));
    // A fixed option changed in the JSON tab: named, and Review stays off.
    f.edit_json(cx, &dialog, |j| {
        j["token_separators"] = json!(["-"]);
        j["fields"][0]["facet"] = json!(true);
    });
    let summary = f.label_of(cx, "schema.issues");
    assert!(
        summary.contains("Token separators cannot be changed after the collection is created."),
        "{summary}"
    );
    assert!(f.submit_disabled(cx));
    f.edit_json(cx, &dialog, |j| {
        j.as_object_mut().unwrap().remove("token_separators");
        j["name"] = json!("novels");
    });
    let summary = f.label_of(cx, "schema.issues");
    assert!(
        summary.contains("Collection name cannot be changed"),
        "{summary}"
    );
    assert!(f.submit_disabled(cx));
    f.edit_json(cx, &dialog, |j| j["name"] = json!("books"));
    assert!(!f.submit_disabled(cx));
}

#[gpui_kit::test]
async fn review_lists_changes_and_consequences(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, false).await;
    let dialog = f.review_library_change(cx);
    let review = f.label_of(cx, "schema.review");
    for part in [
        "pages",
        "author",
        "title",
        "Typesense rewrites all 1,000 documents of books.",
        "reviews.book_author",
    ] {
        assert!(review.contains(part), "{part} missing in {review}");
    }
    f.query(cx, |w| {
        assert!(w.try_find("schema.review.added.pages").is_some());
        assert!(w.try_find("schema.review.dropped.author").is_some());
        assert!(w.try_find("schema.review.changed.title").is_some());
    });
    let body: Value =
        serde_json::from_str(&cx.update(|cx| dialog.read(cx).review_body(cx).unwrap())).unwrap();
    let fields = body["fields"].as_array().unwrap();
    assert!(fields.contains(&json!({"name": "author", "drop": true})));
    assert!(fields.contains(&json!({"name": "title", "drop": true})));
    assert!(
        fields
            .iter()
            .any(|f| f["name"] == "title" && f["facet"] == true)
    );
    assert!(fields.iter().any(|f| f["name"] == "pages"));
    // Unchanged fields and options never travel, so the masked key does not either.
    assert!(!body.to_string().contains("embedding"), "{body}");
    assert_eq!(body.as_object().unwrap().len(), 1, "{body}");
}

#[gpui_kit::test]
async fn review_focus_is_on_back(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, false).await;
    let dialog = f.review_library_change(cx);
    f.query(cx, |w| {
        assert_eq!(w.find("schema.review.back").focused(), Some(true));
        assert_ne!(w.find("schema.review.apply").focused(), Some(true));
    });
    // Enter on Back goes back to the draft as it was.
    f.act(cx, |window, cx| window.press("enter", cx));
    f.query(cx, |w| assert!(w.try_find("schema.review").is_none()));
    assert!(f.updates().is_empty());
    let json = f.json(cx, &dialog);
    assert!(
        json["fields"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["name"] == "pages")
    );
}

#[gpui_kit::test]
async fn empty_diff_disables_review(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, false).await;
    let dialog = f.edit_of(cx, library(), vec![library()]);
    assert!(f.submit_disabled(cx));
    f.act(cx, |window, cx| window.press(SUBMIT, cx));
    f.query(cx, |w| assert!(w.try_find("schema.review").is_none()));
    // A change and its undo: nothing to send again.
    f.edit_json(cx, &dialog, |j| j["fields"][0]["facet"] = json!(true));
    assert!(!f.submit_disabled(cx));
    f.edit_json(cx, &dialog, |j| j["fields"][0]["facet"] = json!(false));
    assert!(f.submit_disabled(cx));
}

#[gpui_kit::test]
async fn changed_embedding_needs_the_key_again(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, false).await;
    let dialog = f.edit_of(cx, library(), vec![library()]);
    f.edit_json(cx, &dialog, |j| {
        j["fields"][3]["embed"]["model_config"]["model_name"] = json!("openai/other")
    });
    f.query(cx, |w| {
        assert_eq!(
            w.find("schema.field.3.secret").label(),
            Some("Enter the key for the embedding model again")
        )
    });
    assert!(f.submit_disabled(cx));
    f.edit_json(cx, &dialog, |j| {
        j["fields"][3]["embed"]["model_config"]["api_key"] = json!("sk-new")
    });
    assert!(!f.submit_disabled(cx));
    // Metadata edited around a masked value would send the mask.
    f.edit_json(cx, &dialog, |j| j["metadata"]["other"] = json!(1));
    let summary = f.label_of(cx, "schema.issues");
    assert!(summary.contains("masked value"), "{summary}");
    assert!(f.submit_disabled(cx));
}

#[gpui_kit::test]
async fn apply_sends_patch_body(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, false).await;
    let id = f.profile.id;
    cx.update(|cx| {
        f.state
            .update(cx, |s, cx| s.open_collection(id, "books".into(), cx))
    });
    wait_for_element(cx, f.handle, "schema.edit".into(), true).await;
    f.query(cx, |w| {
        let edit = w.find("schema.edit");
        assert!(format!("{edit:?}").contains("Edit schema…"), "{edit:?}");
    });
    let gets = f.backend.gets.lock().unwrap().len();
    let lists = *f.backend.lists.lock().unwrap();
    f.act(cx, |window, cx| window.click("schema.edit", cx));
    f.act(cx, |window, cx| window.click("schema.add_field", cx));
    f.act(cx, |window, cx| {
        fill(window, "schema.field.2.name", "pages", cx)
    });
    // In the last row ⌘↩ adds a field; from the JSON tab it reviews.
    f.act(cx, |window, cx| window.press(SHOW_JSON, cx));
    f.act(cx, |window, cx| window.press(SUBMIT, cx));
    f.apply(cx);
    wait_for_element(cx, f.handle, "schema.review".into(), false).await;
    wait_for_element(cx, f.handle, "schema.progress".into(), false).await;
    let updates = f.updates();
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].0, "books");
    assert_eq!(
        updates[0].1,
        json!({"fields": [{"name": "pages", "type": "string"}]})
    );
    f.settle(cx);
    assert!(*f.backend.lists.lock().unwrap() > lists, "the list reloads");
    assert!(
        f.backend.gets.lock().unwrap().len() > gets,
        "the schema tab reloads"
    );
}

#[gpui_kit::test]
async fn progress_shows_schema_change_counts(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, false).await;
    *f.backend.schema_changes.lock().unwrap() = vec![SchemaChange {
        collection: "books".into(),
        validated_docs: 500,
        altered_docs: 200,
    }];
    let release = f.backend.hold_update();
    f.review_library_change(cx);
    f.apply(cx);
    wait_for_label(cx, f.handle, "schema.progress".into(), "Validated 500").await;
    let progress = f.label_of(cx, "schema.progress");
    assert!(
        progress.contains("Validated 500, altered 200 of 1,000 documents"),
        "{progress}"
    );
    // Polled every second while the change runs.
    let calls = *f.backend.schema_change_calls.lock().unwrap();
    cx.executor().advance_clock(Duration::from_secs(1));
    f.settle(cx);
    assert!(*f.backend.schema_change_calls.lock().unwrap() > calls);
    // The change ends: the dialog closes and the poll stops.
    release.send(()).unwrap();
    wait_for_element(cx, f.handle, "schema.progress".into(), false).await;
    let calls = *f.backend.schema_change_calls.lock().unwrap();
    cx.executor().advance_clock(Duration::from_secs(3));
    f.settle(cx);
    assert_eq!(*f.backend.schema_change_calls.lock().unwrap(), calls);
}

#[gpui_kit::test]
async fn stop_waiting_closes_and_says_server_continues(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, false).await;
    let release = f.backend.hold_update();
    f.review_library_change(cx);
    f.apply(cx);
    wait_for_element(cx, f.handle, "schema.progress".into(), true).await;
    let note = f.label_of(cx, "schema.progress.note");
    assert!(note.contains("The server keeps going"), "{note}");
    assert!(note.contains("reloads the collection"), "{note}");
    f.act(cx, |window, cx| window.click("schema.stop_waiting", cx));
    wait_for_element(cx, f.handle, "schema.progress".into(), false).await;
    // The poll stops with the dialog.
    let calls = *f.backend.schema_change_calls.lock().unwrap();
    cx.executor().advance_clock(Duration::from_secs(3));
    f.settle(cx);
    assert_eq!(*f.backend.schema_change_calls.lock().unwrap(), calls);
    // When the server is done, the collection reloads.
    let lists = *f.backend.lists.lock().unwrap();
    release.send(()).unwrap();
    f.settle(cx);
    assert!(
        *f.backend.lists.lock().unwrap() > lists,
        "the list reloads later"
    );
}

#[gpui_kit::test]
async fn server_error_returns_to_review_with_message(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, false).await;
    let message = "Field `pages` cannot be added: some documents have a string there.";
    *f.backend.update_error.lock().unwrap() = Some((400, message.into()));
    f.review_library_change(cx);
    f.apply(cx);
    wait_for_element(cx, f.handle, "schema.server_error".into(), true).await;
    f.query(cx, |w| {
        let shown = w
            .find("schema.server_error")
            .label()
            .unwrap_or_default()
            .to_string();
        assert!(shown.contains(message), "{shown}");
        assert!(w.try_find("schema.review").is_some());
        assert!(w.try_find("schema.progress").is_none());
    });
}

#[gpui_kit::test]
async fn edit_on_read_only_asks_for_unlock(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, true).await;
    f.review_library_change(cx);
    f.apply(cx);
    wait_for_element(cx, f.handle, "danger.input".into(), true).await;
    assert!(f.updates().is_empty());
    let name = f.profile.name.clone();
    f.act(cx, |window, cx| {
        window.click("danger.input".to_string(), cx);
        window.input(&name, cx);
    });
    f.act(cx, |window, cx| window.click("danger.ok".to_string(), cx));
    wait_for_element(cx, f.handle, "danger.input".into(), false).await;
    f.settle(cx);
    assert_eq!(f.updates().len(), 1);
}

#[gpui_kit::test]
async fn edit_shortcut_opens_edit_from_the_list(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, false).await;
    f.act(cx, |window, cx| {
        window.click(format!("sidebar.{}.collections", f.profile.id), cx)
    });
    wait_for_element(cx, f.handle, "collections.row.books".into(), true).await;
    f.act(cx, |window, cx| {
        window.click("collections.row.books", cx);
        window.press(EDIT_SCHEMA, cx);
    });
    f.query(cx, |w| {
        assert_eq!(w.find("schema.name").value(), Some("books"));
        assert_eq!(w.find("schema.submit").label(), Some("Review changes…"));
    });
}
