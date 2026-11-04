// SPDX-License-Identifier: GPL-3.0-or-later

//! The connection dialog for new and edited profiles (design §7.4).

use std::cell::Cell;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonGroup, ButtonVariants as _};
use gpui_kit::component::collapsible::Collapsible;
use gpui_kit::component::dialog::Cancel;
use gpui_kit::component::form::Field;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, Selectable as _, Sizable as _, WindowExt as _,
    h_flex, v_flex,
};
use gpui_kit::{
    App, AppContext as _, Context, Entity, Focusable as _, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, Task, Window, div, prelude::*, px,
};
use rust_i18n::t;
use secrecy::{ExposeSecret as _, SecretString};
use tessera_client::KeyAccess;
use tessera_core::profile::{Environment, Node, Profile, ProfileColor, Protocol};
use uuid::Uuid;

use super::node_rows::{self, NodeRow};
use super::{env_badge, overview};
use crate::actions::{CONNECT_DIALOG, SaveConnection, TestConnection, ToggleKeyMask};
use crate::connection::{ConnectorGlobal, ServerInfo, error_text};
use crate::runtime::Tokio;
use crate::state::{AppState, SaveError};

const COLORS: [(ProfileColor, &str); 6] = [
    (ProfileColor::Gray, "gray"),
    (ProfileColor::Blue, "blue"),
    (ProfileColor::Teal, "teal"),
    (ProfileColor::Violet, "violet"),
    (ProfileColor::Amber, "amber"),
    (ProfileColor::Red, "red"),
];

/// The result of the last Test.
struct Outcome {
    at: Instant,
    elapsed: Duration,
    result: Result<(ServerInfo, KeyAccess), SharedString>,
}

/// Dialog state. Lives as long as the dialog is open; closing it drops a running test.
pub struct ConnectDialog {
    state: Entity<AppState>,
    /// The edited profile, or the new one. Fields the form does not show are kept.
    base: Profile,
    /// Editing a saved profile: an empty key field keeps the stored key.
    editing: bool,
    name: Entity<InputState>,
    color: ProfileColor,
    env: Environment,
    env_name: Entity<InputState>,
    nodes: Vec<NodeRow>,
    key: Entity<InputState>,
    read_only: bool,
    advanced: bool,
    timeout: Entity<InputState>,
    retries: Entity<InputState>,
    /// Inline errors by the test id of the element that shows them.
    errors: HashMap<String, SharedString>,
    outcome: Option<Outcome>,
    /// Profile, typed key and used key of the last passing test.
    passed: Option<(Profile, Option<SecretString>, SecretString)>,
    /// The keychain refused the key; it can still be used for this session.
    keychain_error: Option<(SharedString, SecretString)>,
    save_error: Option<SharedString>,
    busy: bool,
    /// Replacing or dropping it cancels the running test or save.
    task: Option<Task<()>>,
    /// Re-renders once a second so "Tested … ago" stays true.
    tick: Option<Task<()>>,
    subscriptions: Vec<Subscription>,
}

impl ConnectDialog {
    /// Opens the dialog for a new profile, optionally prefilled (a temporary connection
    /// saved as a profile). The prefilled profile gets a fresh id.
    pub fn open_new(
        state: Entity<AppState>,
        prefill: Option<(Profile, Option<SecretString>)>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let (profile, key) = match prefill {
            Some((profile, key)) => (
                Profile {
                    id: Uuid::new_v4(),
                    ..profile
                },
                key,
            ),
            None => (Profile::new("", vec![default_node()]), None),
        };
        Self::open(state, profile, key, false, window, cx);
    }

    /// Opens the dialog for saved profile `id`.
    pub fn open_edit(state: Entity<AppState>, id: Uuid, window: &mut Window, cx: &mut App) {
        let Some(profile) = state.read(cx).profiles.iter().find(|p| p.id == id).cloned() else {
            return;
        };
        Self::open(state, profile, None, true, window, cx);
    }

    fn open(
        state: Entity<AppState>,
        profile: Profile,
        key: Option<SecretString>,
        editing: bool,
        window: &mut Window,
        cx: &mut App,
    ) {
        let view = cx.new(|cx| Self::new(state, profile, key, editing, window, cx));
        let name = view.read(cx).name.clone();
        let first_build = Cell::new(true);
        let title = if editing {
            t!("connect.title_edit")
        } else {
            t!("connect.title_new")
        }
        .to_string();
        window.open_dialog(cx, move |dialog, window, cx| {
            if first_build.replace(false) {
                name.focus_handle(cx).focus(window, cx);
            }
            dialog
                .title(title.clone())
                .w(px(600.))
                // Enter would close the dialog by default; Save is ⌘↩ / Ctrl+↩ instead.
                .on_ok(|_, _, _| false)
                .child(view.clone())
        });
    }

    fn new(
        state: Entity<AppState>,
        profile: Profile,
        key: Option<SecretString>,
        editing: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut input =
            |value: String| cx.new(|cx| InputState::new(window, cx).default_value(value));
        let name = input(profile.name.clone());
        let env_name = input(match &profile.environment {
            Environment::Custom(name) => name.clone(),
            _ => String::new(),
        });
        let timeout = input(profile.timeout_secs.to_string());
        let retries = input(profile.retries.to_string());
        let key_text = key
            .map(|k| k.expose_secret().to_string())
            .unwrap_or_default();
        let key = cx.new(|cx| {
            let input = InputState::new(window, cx)
                .masked(true)
                .default_value(key_text);
            if editing {
                input.placeholder(t!("connect.key_keep").to_string())
            } else {
                input
            }
        });
        let nodes = profile
            .nodes
            .iter()
            .map(|n| NodeRow::new(n, window, cx))
            .collect();
        let mut this = Self {
            state,
            editing,
            name,
            color: profile.color,
            env: profile.environment.clone(),
            env_name,
            nodes,
            key,
            read_only: profile.read_only,
            advanced: false,
            timeout,
            retries,
            errors: HashMap::new(),
            outcome: None,
            passed: None,
            keychain_error: None,
            save_error: None,
            busy: false,
            task: None,
            tick: None,
            subscriptions: Vec::new(),
            base: profile,
        };
        for input in [
            &this.name,
            &this.env_name,
            &this.key,
            &this.timeout,
            &this.retries,
        ]
        .into_iter()
        .cloned()
        .collect::<Vec<_>>()
        {
            this.watch(&input, cx);
        }
        let rows: Vec<_> = this
            .nodes
            .iter()
            .flat_map(|r| [r.host.clone(), r.port.clone(), r.path.clone()])
            .collect();
        for input in rows {
            this.watch(&input, cx);
        }
        this
    }

    /// Re-renders when `input` changes, for the summary and the http warning.
    fn watch(&mut self, input: &Entity<InputState>, cx: &mut Context<Self>) {
        self.subscriptions
            .push(cx.subscribe(input, |_, _, _: &InputEvent, cx| cx.notify()));
    }

    pub(super) fn flip_protocol(&mut self, i: usize, cx: &mut Context<Self>) {
        if let Some(row) = self.nodes.get_mut(i) {
            row.protocol = match row.protocol {
                Protocol::Http => Protocol::Https,
                Protocol::Https => Protocol::Http,
            };
            cx.notify();
        }
    }

    pub(super) fn remove_node(&mut self, i: usize, cx: &mut Context<Self>) {
        if self.nodes.len() > 1 && i < self.nodes.len() {
            self.nodes.remove(i);
            self.errors.retain(|k, _| !k.starts_with("connect.node."));
            cx.notify();
        }
    }

    pub(super) fn add_node(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let row = NodeRow::new(&default_node(), window, cx);
        for input in [row.host.clone(), row.port.clone(), row.path.clone()] {
            self.watch(&input, cx);
        }
        row.host.focus_handle(cx).focus(window, cx);
        self.nodes.push(row);
        cx.notify();
    }

    fn set_env(&mut self, env: Environment, window: &mut Window, cx: &mut Context<Self>) {
        if env.is_prod() {
            self.read_only = true;
        }
        if matches!(env, Environment::Custom(_)) {
            self.env_name.focus_handle(cx).focus(window, cx);
        }
        self.env = env;
        cx.notify();
    }

    fn toggle_key_mask(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.key.update(cx, |k, cx| k.toggle_masked(window, cx));
    }

    /// Reads the form into a profile and the typed key (`None` keeps the stored one).
    /// Records field errors and returns `None` when there are any.
    fn form(&mut self, cx: &App) -> Option<(Profile, Option<SecretString>)> {
        let text = |input: &Entity<InputState>| input.read(cx).value().trim().to_string();
        let mut errors = HashMap::new();
        let mut profile = self.base.clone();
        profile.nodes = Vec::new();
        for (i, row) in self.nodes.iter().enumerate() {
            match row.read(cx) {
                Ok(node) => profile.nodes.push(node),
                Err(e) => {
                    errors.insert(format!("connect.node.{i}.error"), e);
                }
            }
        }
        let name = text(&self.name);
        profile.name = match (name.is_empty(), profile.nodes.first()) {
            (true, Some(node)) => node.label(),
            _ => name,
        };
        profile.color = self.color;
        profile.environment = match &self.env {
            Environment::Custom(_) => {
                let name = text(&self.env_name);
                if name.is_empty() {
                    errors.insert(
                        "connect.env.error".into(),
                        t!("connect.error.env_required").into(),
                    );
                }
                Environment::parse(&name)
            }
            env => env.clone(),
        };
        profile.read_only = self.read_only;
        let mut advanced_error = false;
        match in_range(&text(&self.timeout), 1, 300) {
            Some(v) => profile.timeout_secs = v,
            None => {
                advanced_error = true;
                errors.insert(
                    "connect.timeout.error".into(),
                    t!("connect.error.timeout_range").into(),
                );
            }
        }
        match in_range(&text(&self.retries), 0, 10) {
            Some(v) => profile.retries = v,
            None => {
                advanced_error = true;
                errors.insert(
                    "connect.retries.error".into(),
                    t!("connect.error.retries_range").into(),
                );
            }
        }
        let key = text(&self.key);
        let key = (!key.is_empty()).then(|| SecretString::from(key));
        if key.is_none() && !self.editing {
            errors.insert(
                "connect.key.error".into(),
                t!("connect.error.key_required").into(),
            );
        }
        self.advanced |= advanced_error;
        self.errors = errors;
        self.errors.is_empty().then_some((profile, key))
    }

    /// Tests the form's values; with `save`, saves and connects when the test passes.
    /// A passing test for the same values is not repeated.
    fn run(&mut self, save: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.keychain_error = None;
        self.save_error = None;
        let Some((profile, typed)) = self.form(cx) else {
            self.task = None;
            self.busy = false;
            cx.notify();
            return;
        };
        if save
            && let Some((p, t, key)) = &self.passed
            && *p == profile
            && same_key(t, &typed)
        {
            let key = key.clone();
            self.store(profile, typed, key, window, cx);
            return;
        }
        let id = profile.id;
        let stored = typed.is_none().then(|| {
            let secrets = self.state.read(cx).secrets.clone();
            cx.global::<Tokio>()
                .0
                .spawn_blocking(move || secrets.get(id))
        });
        let connect = cx.global::<ConnectorGlobal>().0.clone();
        self.busy = true;
        self.outcome = None;
        self.passed = None;
        cx.notify();
        self.task = Some(cx.spawn_in(window, async move |this, cx| {
            // `stored` is set exactly when no key was typed.
            let key = match stored {
                None => typed.clone().ok_or_else(String::new),
                Some(job) => match job.await {
                    Ok(Ok(Some(key))) => Ok(key),
                    Ok(Ok(None)) => Err(t!("connect.error.key_required").to_string()),
                    Ok(Err(e)) => {
                        Err(t!("connect.error.keychain", detail = e.to_string()).to_string())
                    }
                    Err(e) => Err(t!("connect.error.keychain", detail = e.to_string()).to_string()),
                },
            };
            let start = Instant::now();
            let result = match key {
                Ok(key) => connect(profile.clone(), key.clone())
                    .await
                    .map(|c| (c, key))
                    .map_err(|e| error_text(&e, &profile)),
                Err(e) => Err(e),
            };
            let elapsed = start.elapsed();
            this.update_in(cx, |view, window, cx| {
                view.busy = false;
                view.outcome = Some(Outcome {
                    at: Instant::now(),
                    elapsed,
                    result: match &result {
                        Ok((c, _)) => Ok((c.info.clone(), c.access)),
                        Err(e) => Err(e.clone().into()),
                    },
                });
                view.start_tick(cx);
                if let Ok((_, key)) = result {
                    view.passed = Some((profile.clone(), typed.clone(), key.clone()));
                    if save {
                        view.store(profile, typed, key, window, cx);
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Saves the profile and its typed key, then connects with `key`.
    fn store(
        &mut self,
        profile: Profile,
        typed: Option<SecretString>,
        key: SecretString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = profile.id;
        let saved = self
            .state
            .update(cx, |s, cx| s.upsert_profile(profile, typed, cx));
        self.busy = true;
        cx.notify();
        self.task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = saved.await;
            this.update_in(cx, |view, window, cx| {
                view.busy = false;
                match result {
                    Ok(()) => view.finish(id, key, window, cx),
                    Err(SaveError::Keychain(e)) => {
                        let text = t!("connect.error.keychain_store", detail = e.to_string());
                        view.keychain_error = Some((text.to_string().into(), key));
                    }
                    Err(SaveError::File(e)) => {
                        view.save_error =
                            Some(t!("connect.error.save_file", detail = e).to_string().into());
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Keeps the refused key in memory, connects and closes.
    fn session_only(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, key)) = self.keychain_error.take() else {
            return;
        };
        let id = self.base.id;
        self.state.update(cx, |s, _| {
            s.session_keys.insert(id, key.clone());
        });
        self.finish(id, key, window, cx);
    }

    /// Reconnects with the saved profile, passing the key so it is not read back.
    fn finish(&mut self, id: Uuid, key: SecretString, window: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            s.disconnect(id, cx);
            s.connect(id, Some(key), cx);
        });
        window.close_dialog(cx);
    }

    fn start_tick(&mut self, cx: &mut Context<Self>) {
        self.tick = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        }));
    }

    fn error(&self, id: &str, cx: &App) -> Option<gpui_kit::AnyElement> {
        self.errors
            .get(id)
            .map(|e| error_line(id.to_string().into(), e.clone(), cx).into_any_element())
    }

    fn render_result(&self, cx: &App) -> Option<impl IntoElement> {
        let outcome = self.outcome.as_ref()?;
        let theme = cx.theme();
        let (lines, color) = match &outcome.result {
            Ok((info, access)) => {
                let role = overview::role_text(info.state);
                let health = if info.healthy {
                    t!(
                        "connect.result.healthy",
                        version = info.version,
                        role = role
                    )
                } else {
                    t!(
                        "connect.result.unhealthy",
                        version = info.version,
                        role = role
                    )
                };
                let access = match access {
                    KeyAccess::Admin => t!("connect.result.admin"),
                    KeyAccess::Restricted => t!("connect.result.restricted"),
                };
                let secs = outcome.at.elapsed().as_secs();
                let ago = match secs {
                    0..5 => t!("connect.result.just_now"),
                    5..60 => t!("connect.result.seconds_ago", n = secs),
                    _ => t!("connect.result.minutes_ago", n = secs / 60),
                };
                let tested = t!(
                    "connect.result.tested",
                    ago = ago,
                    ms = outcome.elapsed.as_millis()
                );
                let color = if info.healthy {
                    theme.success
                } else {
                    theme.warning
                };
                (
                    vec![health.to_string(), access.to_string(), tested.to_string()],
                    color,
                )
            }
            Err(e) => (vec![e.to_string()], theme.danger),
        };
        let label = SharedString::from(lines.join("\n"));
        Some(
            v_flex()
                .id("connect.result")
                .test_support()
                .aria_label(label)
                .text_sm()
                .text_color(color)
                .children(lines.into_iter().map(|l| div().child(l))),
        )
    }
}

impl Render for ConnectDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let field = |label: String| Field::new().label(label);
        let name_label = t!("connect.name").to_string();
        let key_label = t!("connect.api_key").to_string();
        let masked = self.key.read(cx).presentation().is_masked();
        let kbd = |action: &dyn gpui_kit::Action| {
            Kbd::binding_for_action(action, Some(CONNECT_DIALOG), window)
        };
        let colors = h_flex().gap_1().children(COLORS.map(|(color, name)| {
            Button::new(SharedString::from(format!("connect.color.{name}")))
                .ghost()
                .small()
                .selected(self.color == color)
                .accessibility_label(t!(format!("connect.color.{name}")).to_string())
                .child(
                    div()
                        .size_4()
                        .rounded_full()
                        .bg(env_badge::color(color, &theme)),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.color = color;
                    cx.notify();
                }))
        }));
        let envs = [
            (Environment::Local, "local"),
            (Environment::Dev, "dev"),
            (Environment::Staging, "staging"),
            (Environment::Prod, "prod"),
            (Environment::Custom(String::new()), "other"),
        ];
        let custom = matches!(self.env, Environment::Custom(_));
        let env_group = ButtonGroup::new("connect.env")
            .outline()
            .small()
            .children(envs.map(|(env, name)| {
                let selected = std::mem::discriminant(&env) == std::mem::discriminant(&self.env);
                Button::new(SharedString::from(format!("connect.env.{name}")))
                    .label(t!(format!("connect.env.{name}")).to_string())
                    .selected(selected)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.set_env(env.clone(), window, cx);
                    }))
            }));
        let key_hint = if cfg!(target_os = "macos") {
            t!("connect.key_hint.macos")
        } else if cfg!(target_os = "windows") {
            t!("connect.key_hint.windows")
        } else {
            t!("connect.key_hint.linux")
        };
        let summary = t!(
            "connect.advanced_summary",
            timeout = self.timeout.read(cx).value().trim(),
            retries = self.retries.read(cx).value().trim()
        )
        .to_string();
        let remote_http = self.nodes.iter().any(|r| r.is_remote_http(cx));
        v_flex()
            .key_context(CONNECT_DIALOG)
            .on_action(
                cx.listener(|this, _: &TestConnection, window, cx| this.run(false, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SaveConnection, window, cx| this.run(true, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &ToggleKeyMask, window, cx| this.toggle_key_mask(window, cx)),
            )
            .gap_3()
            .child(
                field(name_label.clone()).child(
                    Input::new(&self.name)
                        .id("connect.name")
                        .aria_label(name_label),
                ),
            )
            .child(field(t!("connect.color").to_string()).child(colors))
            .child(
                field(t!("connect.environment").to_string())
                    .child(h_flex().gap_2().child(env_group).when(custom, |el| {
                        el.child(
                            div().w(px(160.)).child(
                                Input::new(&self.env_name)
                                    .id("connect.env.name")
                                    .aria_label(t!("connect.env.name").to_string()),
                            ),
                        )
                    }))
                    .children(self.error("connect.env.error", cx))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(t!("connect.env.prod_hint").to_string()),
                    ),
            )
            .child(
                field(t!("connect.nodes").to_string())
                    .child(node_rows::render(&self.nodes, &self.errors, cx))
                    .when(remote_http, |el| {
                        let text = SharedString::from(t!("connect.http_warning").to_string());
                        el.child(
                            div()
                                .id("connect.http_warning")
                                .test_support()
                                .aria_label(text.clone())
                                .p_2()
                                .rounded_md()
                                .bg(theme.warning.opacity(0.15))
                                .text_sm()
                                .child(text),
                        )
                    }),
            )
            .child(
                field(key_label.clone())
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                div().flex_1().child(
                                    Input::new(&self.key)
                                        .id("connect.key")
                                        .aria_label(key_label),
                                ),
                            )
                            .child(
                                Button::new("connect.key.toggle")
                                    .ghost()
                                    .icon(if masked {
                                        IconName::Eye
                                    } else {
                                        IconName::EyeOff
                                    })
                                    .accessibility_label(
                                        if masked {
                                            t!("connect.key.show")
                                        } else {
                                            t!("connect.key.hide")
                                        }
                                        .to_string(),
                                    )
                                    .children(kbd(&ToggleKeyMask))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.toggle_key_mask(window, cx)
                                    })),
                            ),
                    )
                    .children(self.error("connect.key.error", cx))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(key_hint.to_string()),
                    ),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        Switch::new("connect.read_only")
                            .checked(self.read_only)
                            .label(t!("connect.read_only").to_string())
                            .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                this.read_only = *checked;
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(t!("connect.read_only_hint").to_string()),
                    ),
            )
            .child(
                Collapsible::new()
                    .open(self.advanced)
                    .gap_2()
                    .child(
                        h_flex().child(
                            Button::new("connect.advanced")
                                .ghost()
                                .small()
                                .icon(if self.advanced {
                                    IconName::ChevronDown
                                } else {
                                    IconName::ChevronRight
                                })
                                .label(summary)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.advanced = !this.advanced;
                                    cx.notify();
                                })),
                        ),
                    )
                    .content(
                        h_flex()
                            .gap_3()
                            .items_start()
                            .child(
                                field(t!("connect.timeout").to_string())
                                    .w(px(160.))
                                    .child(
                                        Input::new(&self.timeout)
                                            .id("connect.timeout")
                                            .aria_label(t!("connect.timeout").to_string()),
                                    )
                                    .children(self.error("connect.timeout.error", cx)),
                            )
                            .child(
                                field(t!("connect.retries").to_string())
                                    .w(px(160.))
                                    .child(
                                        Input::new(&self.retries)
                                            .id("connect.retries")
                                            .aria_label(t!("connect.retries").to_string()),
                                    )
                                    .children(self.error("connect.retries.error", cx)),
                            ),
                    ),
            )
            .children(self.render_result(cx))
            .when_some(self.keychain_error.as_ref(), |el, (text, _)| {
                el.child(
                    v_flex()
                        .gap_2()
                        .items_start()
                        .child(error_line(
                            "connect.keychain_error".into(),
                            text.clone(),
                            cx,
                        ))
                        .child(
                            Button::new("connect.session_only")
                                .outline()
                                .small()
                                .label(t!("connect.session_only").to_string())
                                .on_click(
                                    cx.listener(|this, _, window, cx| {
                                        this.session_only(window, cx)
                                    }),
                                ),
                        ),
                )
            })
            .when_some(self.save_error.clone(), |el, text| {
                el.child(error_line("connect.save_error".into(), text, cx))
            })
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("connect.test")
                            .outline()
                            .label(t!("connect.test").to_string())
                            .children(kbd(&TestConnection))
                            .disabled(self.busy)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.run(false, window, cx)),
                            ),
                    )
                    .child(
                        Button::new("connect.cancel")
                            .label(t!("connect.cancel").to_string())
                            .children(Kbd::binding_for_action(&Cancel, Some("Dialog"), window))
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("connect.save")
                            .primary()
                            .label(t!("connect.save").to_string())
                            .children(kbd(&SaveConnection))
                            .loading(self.busy)
                            .disabled(self.busy)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.run(true, window, cx)),
                            ),
                    ),
            )
    }
}

/// An inline error under a field, with its test id.
pub(super) fn error_line(id: SharedString, text: SharedString, cx: &App) -> impl IntoElement {
    div()
        .id(id)
        .test_support()
        .aria_label(text.clone())
        .text_sm()
        .text_color(cx.theme().danger)
        .child(text)
}

/// The node a new profile or an added row starts with.
fn default_node() -> Node {
    Node {
        protocol: Protocol::Http,
        host: "localhost".into(),
        port: 8108,
        path: String::new(),
    }
}

/// `text` as a number within `lo..=hi`.
fn in_range(text: &str, lo: u32, hi: u32) -> Option<u32> {
    text.parse().ok().filter(|v| (lo..=hi).contains(v))
}

fn same_key(a: &Option<SecretString>, b: &Option<SecretString>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a.expose_secret() == b.expose_secret(),
        (None, None) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_are_inclusive() {
        assert_eq!(in_range("1", 1, 300), Some(1));
        assert_eq!(in_range("300", 1, 300), Some(300));
        assert_eq!(in_range("0", 1, 300), None);
        assert_eq!(in_range("301", 1, 300), None);
        assert_eq!(in_range("0", 0, 10), Some(0));
        assert_eq!(in_range("-1", 0, 10), None);
        assert_eq!(in_range("", 0, 10), None);
    }
}
