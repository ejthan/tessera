// SPDX-License-Identifier: GPL-3.0-or-later

//! Edit mode (design §7.7): the checks a draft needs before its review, the review of the
//! diff with its consequences and exact PATCH body, and the progress while the PATCH runs
//! (§4.3).

use std::time::Duration;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::Cancel;
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, SharedString,
    Styled as _, Window, div, prelude::*, px,
};
use rust_i18n::t;
use serde_json::{Map, Value};
use tessera_client::Collection;
use tessera_core::redact::{contains_mask, mask_value};
use tessera_core::schema::{
    Consequence, SchemaDiff, SchemaDraft, diff, diff_with_references, validate,
};
use tessera_widgets::JsonEditor;

use super::form::line;
use super::{Mode, SchemaDialog, Step};
use crate::collections::CollectionsBackendGlobal;
use crate::connection::long_error_text;
use crate::views::overview::grouped;

/// How often the progress asks for the schema change counts.
const POLL_EVERY: Duration = Duration::from_secs(1);
const BODY_HEIGHT: f32 = 200.;

/// The diff under review and the read-only editor with its PATCH body.
pub(super) struct Review {
    diff: SchemaDiff,
    body: Entity<JsonEditor>,
}

/// `c` with its secrets masked as the schema view masks them (design §5.4).
pub(super) fn masked(mut c: Collection) -> Collection {
    let mask = |m: &mut Map<String, Value>| {
        let mut v = Value::Object(std::mem::take(m));
        mask_value(&mut v);
        if let Value::Object(o) = v {
            *m = o;
        }
    };
    c.fields.iter_mut().for_each(|f| mask(&mut f.attributes));
    if let Some(m) = c.metadata.as_mut() {
        mask(m);
    }
    mask(&mut c.extra);
    c
}

/// The sentence the review shows for `c`, a consequence of changing `collection`.
fn consequence_text(c: &Consequence, collection: &str) -> String {
    match c {
        Consequence::RewritesDocuments(n) => {
            t!("schema.review.rewrites", n = grouped(*n), name = collection).to_string()
        }
        Consequence::ClearsDefaultSortingField(field) => {
            t!("schema.review.clears_sorting", field = field).to_string()
        }
        Consequence::BreaksReference { collection, field } => t!(
            "schema.review.breaks_reference",
            collection = collection,
            field = field
        )
        .to_string(),
    }
}

impl SchemaDialog {
    /// Validates the draft; in edit mode also finds what PATCH cannot take or would send
    /// masked, and whether anything changed at all.
    pub(super) fn check_draft(&mut self) {
        let Mode::Edit(old) = &self.mode else {
            self.issues = validate(&self.draft);
            return;
        };
        // Fixed in edit mode; the server clears it when its field goes (design §5.3).
        let mut draft = self.draft.clone();
        draft.default_sorting_field.clear();
        self.issues = validate(&draft);
        let was = SchemaDraft::from_collection(old);
        let now = &self.draft;
        // `diff` ignores these, so an edit in the JSON tab would be dropped silently.
        let fixed = [
            (now.name != was.name, "schema.name"),
            (
                now.token_separators != was.token_separators,
                "schema.opt.token_separators",
            ),
            (
                now.symbols_to_index != was.symbols_to_index,
                "schema.opt.symbols_to_index",
            ),
            (
                now.enable_nested_fields != was.enable_nested_fields,
                "schema.opt.enable_nested_fields",
            ),
            (
                now.default_sorting_field != was.default_sorting_field,
                "schema.opt.default_sorting_field",
            ),
        ];
        let mut problems: Vec<String> = fixed
            .iter()
            .filter(|(changed, _)| *changed)
            .map(|(_, key)| t!("schema.fixed_option", option = t!(*key)).to_string())
            .collect();
        // Other top-level keys (`curation_sets`, newer options): `diff` ignores them too.
        let mut keys: Vec<&String> = now.extra.keys().chain(was.extra.keys()).collect();
        keys.sort();
        keys.dedup();
        problems.extend(
            keys.into_iter()
                .filter(|k| now.extra.get(*k) != was.extra.get(*k))
                .map(|k| t!("schema.fixed_option", option = k).to_string()),
        );
        let d = diff(old, now);
        self.blanked = d
            .added
            .iter()
            .chain(d.changed.iter().map(|c| &c.after))
            // `mask_value` masks at any depth, not only under `embed`.
            .filter(|f| contains_mask(&f.to_value()))
            .map(|f| f.name.clone())
            .collect();
        if let Some(Some(m)) = &d.metadata
            && contains_mask(&Value::Object(m.clone()))
        {
            problems.push(t!("schema.metadata_masked").to_string());
        }
        self.edit_problems = problems;
        self.unchanged = d.is_empty();
    }

    /// The PATCH body the review shows, for tests.
    pub fn review_body(&self, cx: &App) -> Option<String> {
        self.review.as_ref().map(|r| r.body.read(cx).text(cx))
    }

    /// Shows the diff of the draft, with references from the cached collection list.
    pub(super) fn show_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Mode::Edit(old) = &self.mode else {
            return;
        };
        let all = self
            .connection
            .read(cx)
            .collection_cache
            .collections
            .clone();
        let diff = diff_with_references(old, &self.draft, all.as_deref().unwrap_or_default());
        let text = serde_json::to_string_pretty(&diff.patch_body()).unwrap_or_default();
        let body = cx.new(|cx| JsonEditor::new(&text, true, window, cx));
        self.review = Some(Review { diff, body });
        self.step = Step::Review;
        self.server_error = None;
        // Medium danger: focus starts on Back, not on Apply (UX rule 3).
        self.safe_focus.focus(window, cx);
        cx.notify();
    }

    /// Back to the draft as it was.
    pub(super) fn back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.step = Step::Draft;
        self.review = None;
        self.server_error = None;
        self.focus_first(window, cx);
        cx.notify();
    }

    /// Enter in the dialog: activates the focused button of the review or the progress.
    pub(super) fn press_focused(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let safe = self.safe_focus.is_focused(window);
        match self.step {
            Step::Review if safe => self.back(window, cx),
            Step::Review if self.apply_focus.is_focused(window) => self.apply(window, cx),
            Step::Progress if safe => self.stop_waiting(window, cx),
            _ => {}
        }
    }

    /// Apply: asks for the unlock on a read-only connection first, then sends.
    fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || self.step != Step::Review {
            return;
        }
        let this = cx.entity().downgrade();
        self.connection.update(cx, |c, cx| {
            c.ensure_writable(window, cx, move |window, cx| {
                this.update(cx, |d, cx| d.send_patch(window, cx)).ok();
            });
        });
    }

    /// Sends the reviewed PATCH body and polls the schema change counts until it answers.
    fn send_patch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Mode::Edit(old), Some(review)) = (&self.mode, &self.review) else {
            return;
        };
        if self.busy || self.step != Step::Review {
            return;
        }
        let name = old.name.clone();
        let patch = review.diff.patch_body();
        let conn = self.connection.read(cx);
        let profile = conn.profile.clone();
        let Some(client) = conn.connected.as_ref().map(|c| c.client.clone()) else {
            self.server_error = Some(t!("status.not_connected").to_string().into());
            return cx.notify();
        };
        let backend = cx.global::<CollectionsBackendGlobal>().0.clone();
        let update = backend.update(client.clone(), name.clone(), patch);
        self.busy = true;
        self.step = Step::Progress;
        self.progress = None;
        self.server_error = None;
        // "Stop waiting" takes the place of Back.
        self.safe_focus.focus(window, cx);
        let wanted = name.clone();
        // Tagged as polling: one request a second would flood the request log otherwise.
        let client = client.polling();
        self.poll = Some(cx.spawn(async move |this, cx| {
            loop {
                let changes = backend.schema_changes(client.clone()).await;
                let alive = this.update(cx, |d, cx| {
                    // A failed poll keeps the last counts; the PATCH result is what counts.
                    let mine = changes
                        .ok()
                        .and_then(|all| all.into_iter().find(|c| c.collection == wanted));
                    if mine.is_some() {
                        d.progress = mine;
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    return;
                }
                cx.background_executor().timer(POLL_EVERY).await;
            }
        }));
        let connection = self.connection.clone();
        self.task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = update.await;
            // Also after "Stop waiting", when the dialog is gone; after a failure too, since
            // a change that timed out may still have landed.
            connection.update(cx, |c, cx| c.collection_changed(name, cx));
            this.update_in(cx, |d, window, cx| {
                // "Stop waiting" let go of this run; the window may show another dialog now.
                if !d.busy {
                    return;
                }
                d.busy = false;
                d.poll = None;
                match result {
                    Ok(()) => window.close_dialog(cx),
                    Err(e) => {
                        d.step = Step::Review;
                        d.server_error = Some(long_error_text(&e, &profile).into());
                        d.safe_focus.focus(window, cx);
                    }
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// Closes the dialog; the server keeps going and the PATCH's answer still reloads the
    /// collection. The poll stops.
    fn stop_waiting(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(task) = self.task.take() {
            task.detach();
        }
        self.poll = None;
        self.busy = false;
        window.close_dialog(cx);
    }

    pub(super) fn render_review(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let Some(review) = &self.review else {
            return div().into_any_element();
        };
        let d = &review.diff;
        let name = match &self.mode {
            Mode::Edit(old) => old.name.as_str(),
            Mode::Create | Mode::Pick(_) => "",
        };
        let theme = cx.theme();
        let (success, danger, warning, muted, fg, mono) = (
            theme.success,
            theme.danger,
            theme.warning,
            theme.muted_foreground,
            theme.foreground,
            theme.mono_font_family.clone(),
        );
        let added: Vec<(String, String)> = d
            .added
            .iter()
            .map(|f| {
                let id = format!("schema.review.added.{}", f.name);
                (id, format!("+ {}", f.to_value()))
            })
            .collect();
        let dropped: Vec<(String, String)> = d
            .dropped
            .iter()
            .map(|n| (format!("schema.review.dropped.{n}"), format!("− {n}")))
            .collect();
        let changed: Vec<(String, String, String, String)> = d
            .changed
            .iter()
            .map(|c| {
                let id = format!("schema.review.changed.{}", c.name);
                let (before, after) = (c.before.to_value(), c.after.to_value());
                (id, c.name.clone(), before.to_string(), after.to_string())
            })
            .collect();
        let mut options = Vec::new();
        if let Some(sets) = &d.synonym_sets {
            let sets = if sets.is_empty() {
                t!("figure.missing").to_string()
            } else {
                sets.join(", ")
            };
            options.push(t!("schema.review.synonym_sets", sets = sets).to_string());
        }
        if let Some(m) = &d.metadata {
            let m = Value::Object(m.clone().unwrap_or_default()).to_string();
            options.push(t!("schema.review.metadata", metadata = m).to_string());
        }
        let consequences: Vec<String> = d
            .consequences
            .iter()
            .map(|c| consequence_text(c, name))
            .collect();
        let label: Vec<String> = added
            .iter()
            .chain(&dropped)
            .map(|(_, text)| text.clone())
            .chain(
                changed
                    .iter()
                    .map(|(_, n, before, after)| format!("{n}: {before} → {after}")),
            )
            .chain(options.iter().cloned())
            .chain(consequences.iter().cloned())
            .collect();
        let section = |key: &str| div().text_sm().text_color(muted).child(t!(key).to_string());
        let rows = |items: Vec<(String, String)>, color| {
            items
                .into_iter()
                .map(move |(id, text)| line(id.into(), text.into(), color))
        };
        v_flex()
            .gap_3()
            .child(
                v_flex()
                    .id("schema.review")
                    .test_support()
                    .aria_label(SharedString::from(label.join("\n")))
                    .gap_1()
                    .when(!added.is_empty(), |el| {
                        el.child(section("schema.review.added"))
                            .children(rows(added, success))
                    })
                    .when(!dropped.is_empty(), |el| {
                        el.child(section("schema.review.dropped"))
                            .children(rows(dropped, danger))
                    })
                    .when(!changed.is_empty(), |el| {
                        let cell = |text: String| div().flex_1().min_w_0().child(text);
                        el.child(section("schema.review.changed"))
                            .child(
                                h_flex()
                                    .gap_3()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(div().w(px(140.)))
                                    .child(cell(t!("schema.review.before").to_string()))
                                    .child(cell(t!("schema.review.after").to_string())),
                            )
                            .children(changed.into_iter().map(|(id, n, before, after)| {
                                h_flex()
                                    .id(SharedString::from(id))
                                    .test_support()
                                    .aria_label(SharedString::from(format!(
                                        "{n}: {before} → {after}"
                                    )))
                                    .gap_3()
                                    .items_start()
                                    .text_sm()
                                    .font_family(mono.clone())
                                    .child(div().w(px(140.)).text_color(fg).child(n))
                                    .child(cell(before).text_color(danger))
                                    .child(cell(after).text_color(success))
                            }))
                    })
                    .children(
                        options
                            .into_iter()
                            .map(|o| div().text_sm().text_color(fg).child(o)),
                    )
                    .when(!consequences.is_empty(), |el| {
                        el.child(section("schema.review.consequences")).children(
                            consequences.into_iter().enumerate().map(|(i, c)| {
                                line(
                                    format!("schema.review.consequence.{i}").into(),
                                    format!("• {c}").into(),
                                    warning,
                                )
                            }),
                        )
                    }),
            )
            .child(section("schema.review.body"))
            .child(
                div()
                    .id("schema.review.body")
                    .test_support()
                    .h(px(BODY_HEIGHT))
                    .child(review.body.clone()),
            )
            .children(
                self.server_error
                    .clone()
                    .map(|e| line("schema.server_error".into(), e, danger)),
            )
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("schema.review.back")
                            .label(t!("schema.review.back").to_string())
                            // Esc goes back here instead of closing the dialog.
                            .children(Kbd::binding_for_action(&Cancel, Some("Dialog"), window))
                            .track_focus(&self.safe_focus)
                            .on_click(cx.listener(|this, _, window, cx| this.back(window, cx))),
                    )
                    .child(
                        Button::new("schema.review.apply")
                            .danger()
                            .label(t!("schema.review.apply").to_string())
                            .track_focus(&self.apply_focus)
                            .on_click(cx.listener(|this, _, window, cx| this.apply(window, cx))),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn render_progress(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let total = match &self.mode {
            Mode::Edit(old) => old.num_documents,
            Mode::Create | Mode::Pick(_) => 0,
        };
        let (validated, altered) = self
            .progress
            .as_ref()
            .map(|p| (p.validated_docs, p.altered_docs))
            .unwrap_or_default();
        let counts = t!(
            "schema.progress.counts",
            validated = grouped(validated),
            altered = grouped(altered),
            total = grouped(total)
        )
        .to_string();
        let (fg, muted) = (cx.theme().foreground, cx.theme().muted_foreground);
        v_flex()
            .gap_3()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(Spinner::new())
                    .child(line("schema.progress".into(), counts.into(), fg)),
            )
            .child(line(
                "schema.progress.note".into(),
                t!("schema.progress.note").to_string().into(),
                muted,
            ))
            .child(
                h_flex().justify_end().child(
                    Button::new("schema.stop_waiting")
                        .label(t!("schema.stop_waiting").to_string())
                        .track_focus(&self.safe_focus)
                        .on_click(cx.listener(|this, _, window, cx| this.stop_waiting(window, cx))),
                ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn masked_hides_embedding_keys_and_metadata_values() {
        let c: Collection = serde_json::from_value(json!({
            "name": "c", "num_documents": 0, "created_at": 0,
            "fields": [{"name": "v", "type": "float[]", "embed": {"from": ["t"],
                "model_config": {"model_name": "m", "api_key": "sk-1"}}}],
            "metadata": {"value": "secret"}
        }))
        .unwrap_or_default();
        let text = serde_json::to_string(&masked(c)).unwrap_or_default();
        assert!(!text.contains("sk-1") && !text.contains("secret"), "{text}");
        assert!(text.contains("\"model_name\":\"m\""), "{text}");
    }
}
