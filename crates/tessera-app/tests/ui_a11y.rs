// SPDX-License-Identifier: GPL-3.0-or-later

//! Accessibility proofs that run headless: contrast, accessible names and focus order. What a
//! screen reader announces is not tested here: a test window has no accessibility tree.

// Tests unwrap; a panic is the failure we want.
#![allow(clippy::unwrap_used)]

mod common;

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::base::Root;
use gpui_kit::base::test_support::{ElementSnapshot, snapshots};
use gpui_kit::component::{ActiveTheme as _, Theme, ThemeMode};
use gpui_kit::test::{TestAppContextExt as _, TestWindowExt as _};
use gpui_kit::{
    App, AppContext as _, Context, ElementId, Entity, Hsla, Rgba, Role, TestAppContext, Window,
    WindowHandle, px,
};
use tessera_app::actions::{OpenPalette, OpenSettings};
use tessera_app::settings::{appearance_changed, apply};
use tessera_app::state::{AppState, ResourceKind};
use tessera_client::{Collection, Field};
use tessera_core::profile::Profile;
use tessera_core::secrets::{MemoryStore, SecretStore};
use tessera_core::settings::Settings;
use uuid::Uuid;

use common::*;

/// `top` painted over `bottom`.
fn over(top: Rgba, bottom: Rgba) -> Rgba {
    let mix = |t: f32, b: f32| t * top.a + b * (1.0 - top.a);
    Rgba {
        r: mix(top.r, bottom.r),
        g: mix(top.g, bottom.g),
        b: mix(top.b, bottom.b),
        a: 1.0,
    }
}

/// WCAG 2.1 relative luminance.
fn luminance(color: Rgba) -> f32 {
    let linear = |c: f32| {
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(color.r) + 0.7152 * linear(color.g) + 0.0722 * linear(color.b)
}

/// WCAG 2.1 contrast of `fg` on `bg`, with `bg` on the window `background`.
fn contrast(fg: Hsla, bg: Hsla, background: Hsla) -> f32 {
    let bg = over(bg.into(), background.into());
    let (a, b) = (luminance(over(fg.into(), bg)), luminance(bg));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

fn assert_aa(theme: &Theme, name: &str) {
    let window = theme.background;
    let pairs = [
        ("foreground", theme.foreground, window, 4.5),
        ("muted_foreground", theme.muted_foreground, window, 4.5),
        ("danger", theme.danger, window, 4.5),
        ("success", theme.success, window, 4.5),
        ("warning", theme.warning, window, 4.5),
        (
            "muted_foreground on secondary",
            theme.muted_foreground,
            theme.secondary,
            4.5,
        ),
        (
            "muted_foreground on muted",
            theme.muted_foreground,
            theme.muted,
            4.5,
        ),
        ("danger on muted", theme.danger, theme.muted, 4.5),
        (
            "danger_foreground",
            theme.danger_foreground,
            theme.danger,
            4.5,
        ),
        (
            "success_foreground",
            theme.success_foreground,
            theme.success,
            4.5,
        ),
        (
            "warning_foreground",
            theme.warning_foreground,
            theme.warning,
            4.5,
        ),
        (
            "primary_foreground",
            theme.primary_foreground,
            theme.primary,
            4.5,
        ),
        ("ring", theme.ring, window, 3.0),
    ];
    for (pair, fg, bg, least) in pairs {
        let ratio = contrast(fg, bg, window);
        assert!(
            ratio >= least,
            "{name}: {pair} is {ratio:.2}, needs {least}"
        );
    }
}

fn themed(theme: &str) -> Settings {
    Settings {
        theme: theme.into(),
        ..Settings::default()
    }
}

#[gpui_kit::test]
async fn both_themes_meet_aa_also_after_a_system_change(cx: &mut TestAppContext) {
    let handle = open(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        for name in ["light", "dark"] {
            apply(&themed(name), window, cx);
            assert_eq!(cx.theme().mode.is_dark(), name == "dark");
            assert_aa(cx.theme(), name);
        }
        // A system change reloads the kit's colors. The test window's appearance is light.
        let system = themed("system");
        Theme::change(ThemeMode::Dark, Some(window), cx);
        appearance_changed(&system, window, cx);
        assert!(!cx.theme().mode.is_dark());
        assert_aa(cx.theme(), "light after the change");
        // An explicit theme keeps its colors through one.
        apply(&themed("dark"), window, cx);
        appearance_changed(&themed("dark"), window, cx);
        assert_aa(cx.theme(), "dark after the change");
        // The fixes leave the font size alone.
        let large = Settings {
            font_size: Some(18),
            ..themed("system")
        };
        apply(&large, window, cx);
        appearance_changed(&large, window, cx);
        assert_eq!(cx.theme().font_size, px(18.));
        assert_aa(cx.theme(), "large font");
    })
    .unwrap();
}

const NAMED_ROLES: [Role; 11] = [
    Role::Button,
    Role::CheckBox,
    Role::Switch,
    Role::RadioButton,
    Role::Tab,
    Role::TextInput,
    Role::MultilineTextInput,
    Role::PasswordInput,
    Role::SearchInput,
    Role::ComboBox,
    Role::Link,
];

/// Controls the kit draws without a name and offers no way to give one: the skip list of this
/// audit. Everything inside a panel stays audited.
fn kit_only(path: &[ElementId]) -> bool {
    // `TabGroupSkin`: the tabs of the dock with their close buttons, its menu and its toggles.
    let dock_chrome = path
        .iter()
        .rposition(|id| named(id, "tab-panel"))
        .is_some_and(|at| !path[at..].iter().any(|id| named(id, "active-panel")));
    // `Combobox` has no label setter. The field chips' picker opens from a named trigger button.
    let combobox =
        matches!(path.last(), Some(ElementId::NamedInteger(n, _)) if n == "multi-combo-box");
    dock_chrome || combobox
}

/// Visible controls without a name. An input's `aria_label` cannot be told from the placeholder
/// the kit falls back to, so this cannot see an input that only has a placeholder.
fn unnamed(window: &Window) -> Vec<String> {
    let mut found: Vec<String> = snapshots(window)
        .iter()
        .filter(|s| s.visible() && s.role().is_some_and(|r| NAMED_ROLES.contains(&r)))
        .filter(|s| !kit_only(s.path()))
        .filter(|s| s.label().is_none_or(str::is_empty))
        .map(|s| format!("{:?}: {:?}", s.role(), s.path()))
        .collect();
    found.sort();
    found
}

struct Fixture {
    handle: WindowHandle<Root>,
    state: Entity<AppState>,
    id: uuid::Uuid,
    _rt: tokio::runtime::Runtime,
}

fn field(name: &str, kind: &str) -> Field {
    Field {
        name: name.into(),
        r#type: kind.into(),
        attributes: Default::default(),
    }
}

fn collection(name: &str) -> Collection {
    Collection {
        name: name.into(),
        num_documents: 3,
        fields: vec![field("title", "string"), field("year", "int32")],
        ..Default::default()
    }
}

impl Fixture {
    async fn connected(cx: &mut TestAppContext) -> Self {
        fake_backend(cx, Some((vec![collection("books")], vec![])));
        fake_documents(cx);
        fake_transfer(cx);
        fake_relevance(cx);
        fake_admin(cx);
        install_search_fake(cx);
        let profile = Profile::new("test", vec![node()]);
        let id = profile.id;
        fake_connector(cx, move |_| Ok(server("30.2", true)));
        let secrets = Arc::new(MemoryStore::default());
        SecretStore::set(&*secrets, id, &"k".to_string().into()).unwrap();
        let (handle, state, rt) = open_app(cx, vec![profile], secrets);
        cx.update_window(handle.into(), |_, window, cx| window.render_frame(cx))
            .unwrap();
        cx.update(|cx| state.update(cx, |s, cx| s.connect(id, None, cx)));
        cx.wait_for(handle.into(), Duration::from_millis(1000), |window, _| {
            window.try_find("figure.version").is_some()
        })
        .await;
        Self {
            handle,
            state,
            id,
            _rt: rt,
        }
    }

    fn with_state(
        &self,
        cx: &mut TestAppContext,
        f: impl FnOnce(&mut AppState, Uuid, &mut Context<AppState>),
    ) {
        let id = self.id;
        cx.update(|cx| self.state.update(cx, |s, cx| f(s, id, cx)));
    }

    fn audit(&self, cx: &mut TestAppContext) {
        audit(cx, self.handle);
    }
}

fn audit(cx: &mut TestAppContext, handle: WindowHandle<Root>) {
    let found = findings(cx, handle);
    assert!(found.is_empty(), "{}", found.join("\n"));
}

fn findings(cx: &mut TestAppContext, handle: WindowHandle<Root>) -> Vec<String> {
    for _ in 0..3 {
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(200));
    }
    cx.run_until_parked();
    let (found, seen) = cx
        .update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            let visible = snapshots(window)
                .iter()
                .filter(|s| s.visible() && s.role().is_some())
                .count();
            (unnamed(window), visible)
        })
        .unwrap();
    assert!(seen > 0, "nothing rendered");
    found
}

#[gpui_kit::test]
async fn no_control_without_a_name_on_welcome(cx: &mut TestAppContext) {
    let handle = open(cx);
    audit(cx, handle);
}

#[gpui_kit::test]
async fn no_control_without_a_name_in_the_connect_dialog(cx: &mut TestAppContext) {
    let handle = open(cx);
    open_dialog(cx, handle).await;
    audit(cx, handle);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("connect.advanced", cx);
    })
    .unwrap();
    audit(cx, handle);
}

#[gpui_kit::test]
async fn no_control_without_a_name_in_the_overview(cx: &mut TestAppContext) {
    Fixture::connected(cx).await.audit(cx);
}

#[gpui_kit::test]
async fn no_control_without_a_name_in_collections(cx: &mut TestAppContext) {
    let f = Fixture::connected(cx).await;
    f.with_state(cx, |s, id, cx| s.open_collections(id, cx));
    wait_for_element(cx, f.handle, "collections.search".into(), true).await;
    f.audit(cx);
}

#[gpui_kit::test]
async fn no_control_without_a_name_in_documents(cx: &mut TestAppContext) {
    let f = Fixture::connected(cx).await;
    f.with_state(cx, |s, id, cx| s.open_collection(id, "books".into(), cx));
    wait_for_element(cx, f.handle, "documents.goto".into(), true).await;
    f.audit(cx);
}

#[gpui_kit::test]
async fn no_control_without_a_name_in_search(cx: &mut TestAppContext) {
    let f = Fixture::connected(cx).await;
    f.with_state(cx, |s, id, cx| s.open_search(id, None, None, cx));
    wait_for_element(cx, f.handle, "search.previous".into(), true).await;
    f.audit(cx);
}

#[gpui_kit::test]
async fn no_control_without_a_name_in_the_console(cx: &mut TestAppContext) {
    let f = Fixture::connected(cx).await;
    f.with_state(cx, |s, id, cx| s.open_console(id, cx));
    wait_for_element(cx, f.handle, "console.send".into(), true).await;
    f.audit(cx);
}

#[gpui_kit::test]
async fn no_control_without_a_name_in_any_resource_tab(cx: &mut TestAppContext) {
    let f = Fixture::connected(cx).await;
    let mut found = Vec::new();
    for kind in ResourceKind::ALL {
        f.with_state(cx, |s, id, cx| s.open_resource(id, kind, cx));
        let tab = findings(cx, f.handle);
        found.extend(
            tab.into_iter()
                .map(|finding| format!("{kind:?}: {finding}")),
        );
    }
    assert!(found.is_empty(), "{}", found.join("\n"));
}

async fn open_settings(cx: &mut TestAppContext, handle: WindowHandle<Root>) {
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.dispatch_action(Box::new(OpenSettings), cx);
    })
    .unwrap();
    wait_for_element(cx, handle, "settings".into(), true).await;
}

#[gpui_kit::test]
async fn no_control_without_a_name_in_settings(cx: &mut TestAppContext) {
    let handle = open(cx);
    open_settings(cx, handle).await;
    audit(cx, handle);
}

#[gpui_kit::test]
async fn no_control_without_a_name_in_the_palette(cx: &mut TestAppContext) {
    let f = Fixture::connected(cx).await;
    cx.update_window(f.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.dispatch_action(Box::new(OpenPalette), cx);
    })
    .unwrap();
    wait_for_element(cx, f.handle, "palette".into(), true).await;
    f.audit(cx);
}

fn named(id: &ElementId, name: &str) -> bool {
    matches!(id, ElementId::Name(n) if n == name)
}

/// The id of the control a snapshot belongs to: an input's state sits below it.
fn control_id(path: &[ElementId]) -> Option<String> {
    path.iter().rev().find_map(|id| match id {
        ElementId::Name(name) if name != "input-state" => Some(name.to_string()),
        _ => None,
    })
}

/// Presses `tab` until focus is back on `first`, asserting at each step that it stays in the
/// dialog, and that the round reached every enabled control the dialog shows.
fn tabs_through_the_dialog(cx: &mut TestAppContext, handle: WindowHandle<Root>, first: &str) {
    let in_dialog = |s: &ElementSnapshot| s.path().iter().any(|id| named(id, "dialog-0"));
    let control = |s: &&ElementSnapshot| {
        s.visible() && s.role().is_some_and(|r| NAMED_ROLES.contains(&r)) && in_dialog(s)
    };
    let mut visited = BTreeSet::new();
    let mut shown = BTreeSet::new();
    for step in 0..48 {
        cx.run_until_parked();
        let (dialog_holds, at, enabled) = cx
            .update_window(handle.into(), |_, window, cx| {
                window.render_frame(cx);
                let all = snapshots(window);
                let dialog_holds = all
                    .iter()
                    .any(|s| s.role() == Some(Role::Dialog) && s.focused() == Some(true));
                let at = all
                    .iter()
                    .filter(control)
                    .find(|s| s.focused() == Some(true))
                    .and_then(|s| control_id(s.path()));
                let enabled: Vec<String> = all
                    .iter()
                    .filter(control)
                    .filter(|s| s.disabled() != Some(true))
                    .filter_map(|s| control_id(s.path()))
                    .collect();
                (dialog_holds, at, enabled)
            })
            .unwrap();
        assert!(dialog_holds, "focus left the dialog at step {step}");
        let at = at.unwrap_or_else(|| panic!("no control has focus at step {step}"));
        if step == 0 {
            assert_eq!(at, first, "the default focus");
        } else if at == first {
            let missed: Vec<_> = shown.difference(&visited).collect();
            assert!(missed.is_empty(), "tab never reached {missed:?}");
            return;
        }
        visited.insert(at);
        shown.extend(enabled);
        cx.update_window(handle.into(), |_, window, cx| window.press("tab", cx))
            .unwrap();
    }
    panic!("tab did not come back to {first}: {visited:?}");
}

#[gpui_kit::test]
async fn dialogs_tab_through_their_controls_connect(cx: &mut TestAppContext) {
    let handle = open(cx);
    open_dialog(cx, handle).await;
    tabs_through_the_dialog(cx, handle, "connect.name");
}

#[gpui_kit::test]
async fn dialogs_tab_through_their_controls_confirm_danger(cx: &mut TestAppContext) {
    let f = Fixture::connected(cx).await;
    f.with_state(cx, |s, id, cx| s.open_collection(id, "books".into(), cx));
    cx.run_until_parked();
    let act = |cx: &mut TestAppContext, f_: fn(&mut Window, &mut App)| {
        cx.update_window(f.handle.into(), |_, window, cx| {
            window.render_frame(cx);
            f_(window, cx);
        })
        .unwrap();
        cx.run_until_parked();
    };
    act(cx, show_schema);
    act(cx, |window, cx| window.click("schema.delete", cx));
    wait_for_element(cx, f.handle, "danger.input".into(), true).await;
    tabs_through_the_dialog(cx, f.handle, "danger.input");
}

#[gpui_kit::test]
async fn dialogs_tab_through_their_controls_settings(cx: &mut TestAppContext) {
    let handle = open(cx);
    open_settings(cx, handle).await;
    tabs_through_the_dialog(cx, handle, "settings.theme");
}
