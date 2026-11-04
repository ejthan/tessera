// SPDX-License-Identifier: GPL-3.0-or-later

//! The connection dialog for new and edited profiles (design §7.4).
//!
//! Save runs as a sequence of [`Phase`]s: test (unless the current values already passed),
//! store the typed key in the keychain, write the profile, then connect with the tested
//! client. A keychain failure writes nothing; a failed test still allows "Save anyway".

mod advanced;
mod form;
mod node_rows;
mod render;
mod result;

use std::cell::Cell;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::Button;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, WindowExt as _, v_flex};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, Focusable as _, IntoElement,
    ParentElement as _, SharedString, Styled as _, Subscription, Task, Window, div, prelude::*, px,
};
use rust_i18n::t;
use secrecy::{ExposeSecret as _, SecretString};
use tessera_core::profile::{Environment, Node, Profile, ProfileColor, Protocol};
use tessera_core::secrets::SecretError;
use uuid::Uuid;

use self::form::{ADVANCED_ERRORS, FormErrors, FormValues};
use self::node_rows::{NodeRow, RowRef};
use self::result::Outcome;
use crate::connection::{Connected, ConnectorGlobal, error_text};
use crate::runtime::Tokio;
use crate::state::{AppState, SaveError};

/// What happens once the profile is written.
#[derive(Clone)]
enum Then {
    /// Show the connection the test established.
    Connect(Box<Connected>),
    /// "Save anyway" after a failed test: only close.
    Close,
}

/// Where the dialog is in testing and saving.
enum Phase {
    Idle,
    Testing {
        then_save: bool,
    },
    /// The last test passed; `outcome` holds the tested values.
    Passed(Box<Connected>),
    /// The last test failed; Save becomes "Save anyway" while the values are unchanged.
    TestFailed,
    /// Writing the key or the profile. Esc, Test and Save wait for it.
    Saving,
    /// The keychain refused the key. Nothing was written.
    KeychainFailed {
        profile: Box<Profile>,
        key: SecretString,
        then: Then,
        text: SharedString,
    },
    /// The key is stored, but `connections.json` could not be written.
    SaveFailed {
        then: Then,
        text: SharedString,
    },
}

/// Dialog state. Lives as long as the dialog is open; closing it drops a running test.
pub struct ConnectDialog {
    state: Entity<AppState>,
    /// The edited profile, or the new one. Fields the form does not show are kept.
    base: Profile,
    editing: bool,
    name: Entity<InputState>,
    color: ProfileColor,
    env: Environment,
    env_name: Entity<InputState>,
    /// The typed environment name read as prod, to switch read-only on once.
    env_name_prod: bool,
    nodes: Vec<NodeRow>,
    nearest: NodeRow,
    key: Entity<InputState>,
    read_only: bool,
    advanced: bool,
    timeout: Entity<InputState>,
    retries: Entity<InputState>,
    ca_file: Option<PathBuf>,
    allow_insecure: bool,
    errors: FormErrors,
    phase: Phase,
    outcome: Option<Outcome>,
    /// The running test or save. Dropping it cancels the requests.
    task: Option<Task<()>>,
    /// Re-renders once a second so "Tested … ago" stays true.
    tick: Option<Task<()>>,
    subscriptions: Vec<Subscription>,
}

impl ConnectDialog {
    /// Opens the dialog for a new profile, optionally prefilled (a temporary connection
    /// saved as a profile). A prefilled profile keeps its id, so saving turns the temporary
    /// connection into the saved profile; one that matches a saved profile gets a fresh id,
    /// so it can never overwrite that profile.
    pub fn open_new(
        state: Entity<AppState>,
        prefill: Option<(Profile, Option<SecretString>)>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let (mut profile, key) =
            prefill.unwrap_or_else(|| (Profile::new("", vec![default_node()]), None));
        if state.read(cx).profiles.iter().any(|p| p.id == profile.id) {
            profile.id = Uuid::new_v4();
        }
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
            let saving = view.clone();
            dialog
                .title(title.clone())
                .w(px(600.))
                // Enter would close the dialog by default; Save is ⌘↩ / Ctrl+↩ instead.
                .on_ok(|_, _, _| false)
                // A write in flight finishes first, so the dialog never closes half-saved.
                .on_cancel(move |_, _, cx| !saving.read(cx).saving())
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
        let nodes: Vec<_> = profile
            .nodes
            .iter()
            .map(|n| NodeRow::new(n, window, cx))
            .collect();
        let nearest = NodeRow::new(
            &profile.nearest_node.clone().unwrap_or(Node {
                host: String::new(),
                ..default_node()
            }),
            window,
            cx,
        );
        let mut inputs = vec![name.clone(), key.clone(), timeout.clone(), retries.clone()];
        inputs.extend(nodes.iter().chain([&nearest]).flat_map(NodeRow::inputs));
        let mut subscriptions: Vec<_> = inputs.iter().map(|i| Self::watch(i, cx)).collect();
        subscriptions.push(cx.subscribe(&env_name, |this, input, _: &InputEvent, cx| {
            let prod = Environment::parse(&input.read(cx).value()).is_prod();
            if prod && !this.env_name_prod {
                this.read_only = true;
            }
            this.env_name_prod = prod;
            cx.notify();
        }));
        Self {
            state,
            editing,
            name,
            color: profile.color,
            env: profile.environment.clone(),
            env_name,
            env_name_prod: false,
            nodes,
            nearest,
            key,
            read_only: profile.read_only,
            advanced: false,
            timeout,
            retries,
            ca_file: profile.tls.ca_file.clone(),
            allow_insecure: profile.tls.allow_insecure,
            errors: FormErrors::new(),
            phase: Phase::Idle,
            outcome: None,
            task: None,
            tick: None,
            subscriptions,
            base: profile,
        }
    }

    /// Re-renders when `input` changes, for the summary, warnings and the stale result.
    fn watch(input: &Entity<InputState>, cx: &mut Context<Self>) -> Subscription {
        cx.subscribe(input, |_, _, _: &InputEvent, cx| cx.notify())
    }

    fn running(&self) -> bool {
        matches!(self.phase, Phase::Testing { .. } | Phase::Saving)
    }

    fn saving(&self) -> bool {
        matches!(self.phase, Phase::Saving)
    }

    fn flip_protocol(&mut self, which: RowRef, cx: &mut Context<Self>) {
        let row = match which {
            RowRef::Node(i) => self.nodes.get_mut(i),
            RowRef::Nearest => Some(&mut self.nearest),
        };
        if let Some(row) = row {
            row.protocol = match row.protocol {
                Protocol::Http => Protocol::Https,
                Protocol::Https => Protocol::Http,
            };
            cx.notify();
        }
    }

    fn remove_node(&mut self, i: usize, cx: &mut Context<Self>) {
        if self.nodes.len() > 1 && i < self.nodes.len() {
            self.nodes.remove(i);
            self.errors.retain(|k, _| !k.starts_with("connect.node."));
            cx.notify();
        }
    }

    fn add_node(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let row = NodeRow::new(&default_node(), window, cx);
        let watched: Vec<_> = row.inputs().iter().map(|i| Self::watch(i, cx)).collect();
        self.subscriptions.extend(watched);
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

    fn values(&self, cx: &App) -> FormValues {
        let text = |input: &Entity<InputState>| input.read(cx).value().to_string();
        FormValues {
            base: self.base.clone(),
            editing: self.editing,
            name: text(&self.name),
            color: self.color,
            env: self.env.clone(),
            env_name: text(&self.env_name),
            nodes: self.nodes.iter().map(|r| r.values(cx)).collect(),
            nearest: self.nearest.values(cx),
            key: text(&self.key),
            read_only: self.read_only,
            timeout: text(&self.timeout),
            retries: text(&self.retries),
            ca_file: self.ca_file.clone(),
            allow_insecure: self.allow_insecure,
        }
    }

    /// Validates the form, showing its errors (and opening Advanced if one is there).
    fn form(&mut self, cx: &mut Context<Self>) -> Option<(Profile, Option<SecretString>)> {
        let result = form::validate(&self.values(cx));
        self.errors = match &result {
            Ok(_) => FormErrors::new(),
            Err(errors) => errors.clone(),
        };
        self.advanced |= ADVANCED_ERRORS
            .iter()
            .any(|id| self.errors.contains_key(*id));
        cx.notify();
        result.ok()
    }

    /// The note shown while `connections.json` could not be loaded. Nothing would be written
    /// then, so saving is blocked and no key goes to the keychain.
    fn blocked(&self, cx: &App) -> Option<SharedString> {
        let error = self.state.read(cx).load_error.as_ref()?;
        Some(t!("connect.blocked", error = error).to_string().into())
    }

    /// Whether the last test ran for the current values.
    fn fresh(&self, cx: &App) -> bool {
        let Some(outcome) = &self.outcome else {
            return false;
        };
        form::validate(&self.values(cx)).is_ok_and(|(p, typed)| outcome.matches(&p, &typed))
    }

    fn test(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.running() {
            return;
        }
        if let Some(form) = self.form(cx) {
            self.start_test(form, false, window, cx);
        }
    }

    /// Saves: reuses a test of the current values, else tests first.
    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.running() || self.blocked(cx).is_some() {
            return;
        }
        let Some((profile, typed)) = self.form(cx) else {
            return;
        };
        let fresh = self
            .outcome
            .as_ref()
            .is_some_and(|o| o.matches(&profile, &typed));
        let then = match &self.phase {
            _ if !fresh => None,
            Phase::Passed(connected) => Some(Then::Connect(connected.clone())),
            Phase::TestFailed => Some(Then::Close),
            Phase::KeychainFailed { then, .. } | Phase::SaveFailed { then, .. } => {
                Some(then.clone())
            }
            Phase::Idle | Phase::Testing { .. } | Phase::Saving => None,
        };
        match then {
            Some(then) => self.persist(profile, typed, then, window, cx),
            None => self.start_test((profile, typed), true, window, cx),
        }
    }

    fn start_test(
        &mut self,
        (profile, typed): (Profile, Option<SecretString>),
        then_save: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = profile.id;
        // Without a typed key: the session key first, then the keychain.
        let known = typed
            .clone()
            .or_else(|| self.state.read(cx).session_keys.get(&id).cloned());
        let stored = known.is_none().then(|| {
            let secrets = self.state.read(cx).secrets.clone();
            cx.global::<Tokio>()
                .0
                .spawn_blocking(move || secrets.get(id))
        });
        let connect = cx.global::<ConnectorGlobal>().0.clone();
        let hook = self.state.read(cx).log_hook(&profile);
        self.phase = Phase::Testing { then_save };
        self.outcome = None;
        cx.notify();
        self.task = Some(cx.spawn_in(window, async move |this, cx| {
            // `stored` is set exactly when no key is known.
            let key = match stored {
                None => known.ok_or_else(String::new),
                Some(job) => match job.await {
                    Ok(Ok(Some(key))) => Ok(key),
                    Ok(Ok(None)) => Err(t!("connect.error.key_required").to_string()),
                    Ok(Err(e)) => Err(keychain_read(&e)),
                    Err(e) => Err(keychain_read(&SecretError::new(e.to_string()))),
                },
            };
            let start = Instant::now();
            let result = match key {
                Ok(key) => connect(profile.clone(), key, hook)
                    .await
                    .map_err(|e| error_text(&e, &profile)),
                Err(e) => Err(e),
            };
            let elapsed = start.elapsed();
            this.update_in(cx, |view, window, cx| {
                view.outcome = Some(Outcome {
                    at: Instant::now(),
                    elapsed,
                    profile: profile.clone(),
                    typed: typed.clone(),
                    result: match &result {
                        Ok(c) => Ok((c.info.clone(), c.access)),
                        Err(e) => Err(e.clone().into()),
                    },
                });
                view.start_tick(cx);
                match result {
                    Ok(connected) if then_save => {
                        let connected = Box::new(connected);
                        view.phase = Phase::Passed(connected.clone());
                        view.persist(profile, typed, Then::Connect(connected), window, cx);
                    }
                    Ok(connected) => view.phase = Phase::Passed(Box::new(connected)),
                    Err(_) => view.phase = Phase::TestFailed,
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Stores the typed key, then writes the profile. A keychain failure writes nothing.
    fn persist(
        &mut self,
        profile: Profile,
        typed: Option<SecretString>,
        then: Then,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = profile.id;
        let stored = typed.map(|key| {
            let secrets = self.state.read(cx).secrets.clone();
            let job_key = key.clone();
            let job = cx
                .global::<Tokio>()
                .0
                .spawn_blocking(move || secrets.set(id, &job_key));
            (key, job)
        });
        self.phase = Phase::Saving;
        cx.notify();
        self.task = Some(cx.spawn_in(window, async move |this, cx| {
            if let Some((key, job)) = stored {
                let result = job
                    .await
                    .unwrap_or_else(|e| Err(SecretError::new(e.to_string())));
                if let Err(e) = result {
                    let text = t!("connect.error.keychain_store", detail = e.to_string());
                    this.update(cx, |view, cx| {
                        view.phase = Phase::KeychainFailed {
                            profile: Box::new(profile),
                            key,
                            then,
                            text: text.to_string().into(),
                        };
                        cx.notify();
                    })
                    .ok();
                    return;
                }
                this.update(cx, |view, cx| {
                    // The keychain holds the key now; an older session key would win.
                    view.state.update(cx, |s, _| s.session_keys.remove(&id));
                })
                .ok();
            }
            this.update_in(cx, |view, window, cx| view.write(profile, then, window, cx))
                .ok();
        }));
    }

    /// "Use for this session only": writes the profile and keeps the key in memory.
    fn session_only(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Phase::KeychainFailed {
            profile, key, then, ..
        } = std::mem::replace(&mut self.phase, Phase::Saving)
        else {
            return;
        };
        let id = profile.id;
        self.state.update(cx, |s, _| s.session_keys.insert(id, key));
        self.write(*profile, then, window, cx);
    }

    /// Writes the profile without touching the keychain, then finishes.
    fn write(&mut self, profile: Profile, then: Then, window: &mut Window, cx: &mut Context<Self>) {
        let saved = self
            .state
            .update(cx, |s, cx| s.upsert_profile(profile.clone(), None, cx));
        self.phase = Phase::Saving;
        cx.notify();
        self.task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = saved.await;
            this.update_in(cx, |view, window, cx| {
                match result {
                    Ok(()) => view.finish(profile, then, window, cx),
                    Err(e) => {
                        let text = match e {
                            SaveError::File(e) => t!("connect.error.save_file", detail = e),
                            SaveError::Keychain(e) => {
                                t!("connect.error.keychain_store", detail = e.to_string())
                            }
                        };
                        view.phase = Phase::SaveFailed {
                            then,
                            text: text.to_string().into(),
                        };
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Shows the tested connection (no second probe round), or after "Save anyway" closes the
    /// profile's open connection, and closes the dialog.
    fn finish(
        &mut self,
        profile: Profile,
        then: Then,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.phase = Phase::Idle;
        let id = profile.id;
        self.state.update(cx, |s, cx| match then {
            Then::Connect(connected) => {
                s.add_connected(profile, *connected, cx);
                s.last_used = Some(id);
                s.save(cx).detach();
            }
            // The new values failed their test, so an open connection with the old profile
            // and client must not stay: the title bar and the read-only guard read it.
            Then::Close => s.disconnect(id, cx),
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

    fn error(&self, id: &str, cx: &App) -> Option<AnyElement> {
        self.errors
            .get(id)
            .map(|e| error_line(id.to_string().into(), e.clone(), cx).into_any_element())
    }

    /// The keychain and file errors with their actions.
    fn render_save_error(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        match &self.phase {
            Phase::KeychainFailed { text, .. } => Some(
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
                                cx.listener(|this, _, window, cx| this.session_only(window, cx)),
                            ),
                    )
                    .into_any_element(),
            ),
            Phase::SaveFailed { text, .. } => {
                Some(error_line("connect.save_error".into(), text.clone(), cx).into_any_element())
            }
            _ => None,
        }
    }
}

/// An inline error under a field, with its test id.
fn error_line(id: SharedString, text: SharedString, cx: &App) -> impl IntoElement {
    div()
        .id(id)
        .test_support()
        .aria_label(text.clone())
        .text_sm()
        .text_color(cx.theme().danger)
        .child(text)
}

fn keychain_read(e: &SecretError) -> String {
    t!("connect.error.keychain", detail = e.to_string()).to_string()
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
