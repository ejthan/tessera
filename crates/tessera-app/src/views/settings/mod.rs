// SPDX-License-Identifier: GPL-3.0-or-later

//! The settings dialog: Appearance, Defaults, Privacy, Storage and About. Plain rows, because the kit's `Settings` picks
//! its pages from a sidebar whose items take no focus, so a keyboard reaches the first page only.

use std::cell::Cell;
use std::ops::RangeInclusive;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::theme::ThemeRegistry;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IndexPath, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::{
    AnyElement, App, AppContext as _, ClipboardItem, Context, Entity, FocusHandle, Focusable as _,
    IntoElement, ParentElement as _, Render, ScrollHandle, SharedString, Styled as _, Subscription,
    Window, div, prelude::*, px,
};
use rust_i18n::t;
use tessera_core::settings::{Density, Language, POLL_CHOICES, RELEASES_PAGE, Settings};
use tessera_core::store;
use tessera_core::transfer::MAX_BATCH;

use tessera_widgets::confirm;

use crate::actions::{OpenLogs, SETTINGS};
use crate::jobs::JobsGlobal;
use crate::relevance::RelevanceBackendGlobal;
use crate::state::{AppState, SettingsProblem, UpdateCheck};

/// Width of the dialog.
const WIDTH: f32 = 560.;
/// Height the dialog keeps free of its rows: its title, its padding and a margin.
const DIALOG_CHROME: f32 = 200.;
/// Width of the control of a row.
const CONTROL_WIDTH: f32 = 200.;
/// The smallest font size; the options follow it up to 20.
const FIRST_FONT_SIZE: u8 = 12;
const FONT_SIZES: usize = 9;
/// Children of the scroller past Defaults, for the focus to scroll to.
const PRIVACY_ROW: usize = 13;
const STORAGE_ROW: usize = 15;
const ABOUT_ROW: usize = 23;

type Choice = Entity<SelectState<Vec<String>>>;

/// The dialog. Each change applies and saves at once, so it has no Save.
pub struct SettingsDialog {
    state: Entity<AppState>,
    theme: Choice,
    font_size: Choice,
    density: Choice,
    language: Choice,
    poll: Choice,
    per_page: Entity<InputState>,
    batch_size: Entity<InputState>,
    confirm_above: Entity<InputState>,
    /// Each control's focus and its row, so Tab never leaves the focus below the fold.
    rows: Vec<(FocusHandle, usize)>,
    focused_row: Option<usize>,
    scroll: ScrollHandle,
    focus_handle: FocusHandle,
    /// The controls of the sections after Defaults, each with its row.
    update_focus: FocusHandle,
    check_focus: FocusHandle,
    logs_focus: FocusHandle,
    clear_focus: FocusHandle,
    copy_focus: [FocusHandle; 3],
    releases_focus: FocusHandle,
    /// What the last Clear cache did.
    cleared: Option<usize>,
    _subscriptions: Vec<Subscription>,
}

fn per_page_range() -> RangeInclusive<u64> {
    10..=u64::from(tessera_core::documents::PER_PAGE)
}

fn batch_range() -> RangeInclusive<u64> {
    1..=u64::from(MAX_BATCH)
}

const CONFIRM_RANGE: RangeInclusive<u64> = 1..=100_000;

fn parsed(input: &Entity<InputState>, range: &RangeInclusive<u64>, cx: &App) -> Option<u64> {
    let value = input.read(cx).value().trim().parse().ok()?;
    range.contains(&value).then_some(value)
}

impl SettingsDialog {
    /// Opens the dialog, focus on its first control.
    pub fn open(state: Entity<AppState>, window: &mut Window, cx: &mut App) {
        let view = cx.new(|cx| Self::new(state, window, cx));
        let first_build = Cell::new(true);
        window.open_dialog(cx, move |dialog, window, cx| {
            if first_build.replace(false) {
                let theme = view.read(cx).theme.clone();
                theme.update(cx, |theme, cx| theme.focus(window, cx));
            }
            dialog
                .title(t!("settings.title").to_string())
                .w(px(WIDTH))
                .child(view.clone())
        });
    }

    fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings = state.read(cx).settings.clone();
        let mut themes: Vec<String> = ["system", "light", "dark"].map(String::from).to_vec();
        themes.extend(
            ThemeRegistry::global(cx)
                .sorted_themes()
                .into_iter()
                .filter(|config| !config.is_default)
                .map(|config| config.name.to_string()),
        );
        let theme_labels: Vec<String> = themes
            .iter()
            .enumerate()
            .map(|(ix, name)| match ix {
                0 => t!("settings.theme.system").to_string(),
                1 => t!("settings.theme.light").to_string(),
                2 => t!("settings.theme.dark").to_string(),
                _ => name.clone(),
            })
            .collect();
        let font_labels: Vec<String> =
            std::iter::once(t!("settings.font_size.default").to_string())
                .chain((0..FONT_SIZES as u8).map(|n| (FIRST_FONT_SIZE + n).to_string()))
                .collect();
        let density_labels = vec![
            t!("settings.density.comfortable").to_string(),
            t!("settings.density.compact").to_string(),
        ];
        let language_labels = vec![
            t!("settings.language.system").to_string(),
            t!("settings.language.en").to_string(),
            t!("settings.language.de").to_string(),
        ];
        let poll_labels: Vec<String> = POLL_CHOICES
            .iter()
            .map(|s| t!("overview.interval", s = s).to_string())
            .collect();

        let mut choice = |labels: &[String], current: usize| {
            let row = IndexPath::default().row(current);
            cx.new(|cx| SelectState::new(labels.to_vec(), Some(row), window, cx))
        };
        let theme = choice(
            &theme_labels,
            themes
                .iter()
                .position(|t| *t == settings.theme)
                .unwrap_or(0),
        );
        let font_size = choice(
            &font_labels,
            settings
                .font_size
                .map_or(0, |n| (n - i64::from(FIRST_FONT_SIZE)) as usize + 1),
        );
        let density = choice(&density_labels, settings.density as usize);
        let language = choice(&language_labels, settings.language as usize);
        let poll = choice(
            &poll_labels,
            POLL_CHOICES
                .iter()
                .position(|s| *s == settings.poll_seconds)
                .unwrap_or(1),
        );
        let mut number =
            |value: String| cx.new(|cx| InputState::new(window, cx).default_value(value));
        let per_page = number(settings.per_page.to_string());
        let batch_size = number(settings.batch_size.to_string());
        let confirm_above = number(settings.confirm_above.to_string());

        let update_focus = cx.focus_handle();
        let (check_focus, logs_focus) = (cx.focus_handle(), cx.focus_handle());
        let copy_focus = [cx.focus_handle(), cx.focus_handle(), cx.focus_handle()];
        let (clear_focus, releases_focus) = (cx.focus_handle(), cx.focus_handle());
        // Row numbers are children of the scroller.
        let rows = vec![
            (theme.focus_handle(cx), 1),
            (font_size.focus_handle(cx), 2),
            (density.focus_handle(cx), 3),
            (language.focus_handle(cx), 4),
            (per_page.focus_handle(cx), 7),
            (batch_size.focus_handle(cx), 8),
            (poll.focus_handle(cx), 9),
            (confirm_above.focus_handle(cx), 10),
            (update_focus.clone(), PRIVACY_ROW - 1),
            (check_focus.clone(), PRIVACY_ROW),
            (logs_focus.clone(), STORAGE_ROW),
            (clear_focus.clone(), STORAGE_ROW + 1),
            (copy_focus[0].clone(), STORAGE_ROW + 2),
            (copy_focus[1].clone(), STORAGE_ROW + 3),
            (copy_focus[2].clone(), STORAGE_ROW + 4),
            (releases_focus.clone(), ABOUT_ROW),
        ];
        let subscriptions = vec![
            Self::on_choice(&theme, theme_labels, window, cx, move |s, ix| {
                s.theme = themes[ix].clone();
            }),
            Self::on_choice(&font_size, font_labels, window, cx, |s, ix| {
                s.font_size = (ix > 0).then(|| i64::from(FIRST_FONT_SIZE) + ix as i64 - 1);
            }),
            Self::on_choice(&density, density_labels, window, cx, |s, ix| {
                s.density = [Density::Comfortable, Density::Compact][ix];
            }),
            Self::on_choice(&language, language_labels, window, cx, |s, ix| {
                s.language = [Language::System, Language::En, Language::De][ix];
            }),
            Self::on_choice(&poll, poll_labels, window, cx, |s, ix| {
                s.poll_seconds = POLL_CHOICES[ix];
            }),
            Self::on_number(&per_page, per_page_range(), window, cx, |s, n| {
                s.per_page = n as u32;
            }),
            Self::on_number(&batch_size, batch_range(), window, cx, |s, n| {
                s.batch_size = n as u32;
            }),
            Self::on_number(&confirm_above, CONFIRM_RANGE, window, cx, |s, n| {
                s.confirm_above = n;
            }),
        ];
        Self {
            state,
            theme,
            font_size,
            density,
            language,
            poll,
            per_page,
            batch_size,
            confirm_above,
            rows,
            focused_row: None,
            scroll: ScrollHandle::new(),
            focus_handle: cx.focus_handle(),
            update_focus,
            check_focus,
            logs_focus,
            clear_focus,
            copy_focus,
            releases_focus,
            cleared: None,
            _subscriptions: subscriptions,
        }
    }

    fn on_choice(
        select: &Choice,
        labels: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
        edit: impl Fn(&mut Settings, usize) + 'static,
    ) -> Subscription {
        cx.subscribe_in(
            select,
            window,
            move |this, _, event: &SelectEvent<Vec<String>>, window, cx| {
                if let SelectEvent::Confirm(Some(label)) = event
                    && let Some(ix) = labels.iter().position(|l| l == label)
                {
                    this.change(|s| edit(s, ix), window, cx);
                }
            },
        )
    }

    fn on_number(
        input: &Entity<InputState>,
        range: RangeInclusive<u64>,
        window: &mut Window,
        cx: &mut Context<Self>,
        edit: impl Fn(&mut Settings, u64) + 'static,
    ) -> Subscription {
        cx.subscribe_in(
            input,
            window,
            move |this, input, event: &InputEvent, window, cx| {
                if !matches!(event, InputEvent::Change) {
                    return;
                }
                match parsed(input, &range, cx) {
                    Some(n) => this.change(|s| edit(s, n), window, cx),
                    // The row says what it takes; nothing changes.
                    None => cx.notify(),
                }
            },
        )
    }

    fn change(
        &mut self,
        edit: impl FnOnce(&mut Settings),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let before = self.state.read(cx).settings.language;
        let settings = self.state.update(cx, |state, cx| {
            state.change_settings(edit, cx);
            state.settings.clone()
        });
        if settings.language != before {
            crate::i18n::init(settings.language);
        }
        crate::settings::apply(&settings, window, cx);
        cx.notify();
    }
}

impl SettingsDialog {
    /// Asks first, in plain words, then clears off this thread.
    fn clear_cache(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let view = cx.entity().downgrade();
        confirm::open(
            t!("settings.clear_cache").to_string(),
            t!("settings.clear_cache.confirm").to_string(),
            t!("settings.clear_cache.action").to_string(),
            move |_, cx| {
                view.update(cx, |view, cx| view.run_clear_cache(cx)).ok();
            },
            window,
            cx,
        );
    }

    fn run_clear_cache(&mut self, cx: &mut Context<Self>) {
        let held = cx.global::<JobsGlobal>().0.read(cx).held_temps();
        let done = cx.global::<RelevanceBackendGlobal>().0.clear_copies(held);
        let state = self.state.clone();
        cx.spawn(async move |this, cx| {
            let count = done.await;
            this.update(cx, |this, cx| {
                for connection in state.read(cx).connections.clone() {
                    connection.update(cx, |c, cx| c.invalidate_collections(cx));
                }
                this.cleared = Some(count);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn update_note(&self, cx: &App) -> Option<(String, bool)> {
        let text = match self.state.read(cx).update_check.as_ref()? {
            UpdateCheck::Running => t!("settings.update.running"),
            UpdateCheck::Current => {
                t!(
                    "settings.update.current",
                    version = env!("CARGO_PKG_VERSION")
                )
            }
            UpdateCheck::Newer(tag) => t!(
                "settings.update.newer",
                version = tag.as_str(),
                url = RELEASES_PAGE
            ),
            UpdateCheck::NoRelease => t!("settings.update.no_release"),
            UpdateCheck::UnknownTag(tag) => t!("settings.update.unknown", tag = tag.as_str()),
            UpdateCheck::Failed(detail) => {
                return Some((
                    t!("settings.update.failed", detail = detail.as_str()).to_string(),
                    true,
                ));
            }
        };
        Some((text.to_string(), false))
    }

    /// Its path, with Copy.
    fn path_row(
        id: &'static str,
        path: Option<std::path::PathBuf>,
        focus: &FocusHandle,
        cx: &App,
    ) -> Option<AnyElement> {
        let path = path?.display().to_string();
        let copy = path.clone();
        Some(
            h_flex()
                .gap_2()
                .justify_between()
                .child(
                    v_flex().min_w_0().child(t!(id).to_string()).child(
                        div()
                            .id(SharedString::from(format!("{id}.path")))
                            .test_support()
                            .aria_label(SharedString::from(path.clone()))
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(path),
                    ),
                )
                .child(
                    Button::new(SharedString::from(format!("{id}.copy")))
                        .small()
                        .track_focus(focus)
                        .label(t!("settings.copy").to_string())
                        .on_click(move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
                        }),
                )
                .into_any_element(),
        )
    }
}

/// A heading of a section.
fn heading(text: String, cx: &App) -> impl IntoElement {
    div()
        .pt_2()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(text)
}

/// A select of a row, named by the row's label.
fn select(id: &'static str, state: &Choice) -> AnyElement {
    Select::new(state)
        .id(id)
        .small()
        .accessibility_label(t!(id).to_string())
        .into_any_element()
}

/// A number input of a row, named by the row's label.
fn number(id: &'static str, input: &Entity<InputState>) -> AnyElement {
    Input::new(input)
        .id(id)
        .small()
        .aria_label(t!(id).to_string())
        .into_any_element()
}

/// One row: its label, under the key `id`, its control, and a note under them when there is one.
fn row(
    id: &'static str,
    control: AnyElement,
    note: Option<(String, bool)>,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    v_flex()
        .gap_1()
        .child(
            h_flex()
                .gap_3()
                .justify_between()
                .child(div().min_w_0().child(t!(id).to_string()))
                .child(div().w(px(CONTROL_WIDTH)).child(control)),
        )
        .children(note.map(|(text, bad)| {
            let text = SharedString::from(text);
            div()
                .id(SharedString::from(format!("{id}.note")))
                .test_support()
                .aria_label(text.clone())
                .text_xs()
                .text_color(if bad {
                    theme.danger
                } else {
                    theme.muted_foreground
                })
                .child(text)
        }))
        .into_any_element()
}

impl Render for SettingsDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self
            .rows
            .iter()
            .find(|(handle, _)| handle.contains_focused(window, cx));
        let focused = focused.map(|(_, row)| *row);
        if focused != self.focused_row {
            self.focused_row = focused;
            focused
                .into_iter()
                .for_each(|row| self.scroll.scroll_to_item(row));
        }
        let error = self.state.read(cx).settings_error.clone();
        let range = |input: &Entity<InputState>, range: RangeInclusive<u64>, cx: &App| {
            parsed(input, &range, cx).is_none().then(|| {
                let text = t!("settings.range", min = range.start(), max = range.end());
                (text.to_string(), true)
            })
        };
        let label = |key: &str| t!(key).to_string();
        let per_page_note = range(&self.per_page, per_page_range(), cx);
        let batch_note = range(&self.batch_size, batch_range(), cx);
        let confirm_note = range(&self.confirm_above, CONFIRM_RANGE, cx)
            .unwrap_or((label("settings.confirm_above.note"), false));
        let update_on = self.state.read(cx).settings.update_check;
        let checking = self.state.read(cx).update_check == Some(UpdateCheck::Running);
        let about = t!(
            "settings.about.version",
            version = env!("CARGO_PKG_VERSION")
        )
        .to_string();
        let rows = v_flex()
            .id("settings.rows")
            .gap_2()
            .max_h(window.viewport_size().height - px(DIALOG_CHROME))
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(heading(label("settings.appearance"), cx))
            .child(row(
                "settings.theme",
                select("settings.theme", &self.theme),
                None,
                cx,
            ))
            .child(row(
                "settings.font_size",
                select("settings.font_size", &self.font_size),
                None,
                cx,
            ))
            .child(row(
                "settings.density",
                select("settings.density", &self.density),
                None,
                cx,
            ))
            .child(row(
                "settings.language",
                select("settings.language", &self.language),
                Some((label("settings.language.note"), false)),
                cx,
            ))
            .child(heading(label("settings.defaults"), cx))
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(label("settings.defaults.note")),
            )
            .child(row(
                "settings.per_page",
                number("settings.per_page", &self.per_page),
                per_page_note,
                cx,
            ))
            .child(row(
                "settings.batch_size",
                number("settings.batch_size", &self.batch_size),
                batch_note,
                cx,
            ))
            .child(row(
                "settings.poll",
                select("settings.poll", &self.poll),
                None,
                cx,
            ))
            .child(row(
                "settings.confirm_above",
                number("settings.confirm_above", &self.confirm_above),
                Some(confirm_note),
                cx,
            ))
            .child(heading(label("settings.privacy"), cx))
            .child(
                v_flex()
                    .track_focus(&self.update_focus)
                    .gap_1()
                    .child(
                        Switch::new("settings.update_check")
                            .label(label("settings.update_check"))
                            .checked(update_on)
                            .on_click(cx.listener(|this, on: &bool, window, cx| {
                                let on = *on;
                                this.change(|s| s.update_check = on, window, cx);
                            })),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(label("settings.update_check.note")),
                    ),
            )
            .child(row(
                "settings.check_now",
                Button::new("settings.check_now.button")
                    .small()
                    .track_focus(&self.check_focus)
                    .label(label("settings.check_now"))
                    .disabled(checking)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.state.update(cx, |s, cx| s.check_for_update(cx));
                    }))
                    .into_any_element(),
                self.update_note(cx),
                cx,
            ))
            .child(heading(label("settings.storage"), cx))
            .child(row(
                "settings.logs",
                Button::new("settings.open_logs")
                    .small()
                    .track_focus(&self.logs_focus)
                    .label(label("settings.open_logs"))
                    .children(Kbd::binding_for_action(&OpenLogs, Some(SETTINGS), window))
                    .on_click(|_, window, cx| window.dispatch_action(Box::new(OpenLogs), cx))
                    .into_any_element(),
                None,
                cx,
            ))
            .child(row(
                "settings.clear_cache",
                Button::new("settings.clear_cache.button")
                    .small()
                    .track_focus(&self.clear_focus)
                    .label(label("settings.clear_cache"))
                    .on_click(cx.listener(|this, _, window, cx| this.clear_cache(window, cx)))
                    .into_any_element(),
                self.cleared.map(|count| {
                    let text = t!("settings.clear_cache.done", count = count);
                    (text.to_string(), false)
                }),
                cx,
            ))
            .children(Self::path_row(
                "settings.path.config",
                store::config_dir(),
                &self.copy_focus[0],
                cx,
            ))
            .children(Self::path_row(
                "settings.path.data",
                store::data_dir(),
                &self.copy_focus[1],
                cx,
            ))
            .children(Self::path_row(
                "settings.path.logs",
                store::logs_dir(),
                &self.copy_focus[2],
                cx,
            ))
            .child(heading(label("settings.about"), cx))
            .child(
                div()
                    .id("settings.about")
                    .test_support()
                    .aria_label(SharedString::from(about.clone()))
                    .text_sm()
                    .child(about),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(label("welcome.trademark")),
            )
            .child(
                h_flex().child(
                    Button::new("settings.releases")
                        .link()
                        .track_focus(&self.releases_focus)
                        .label(label("settings.releases"))
                        .on_click(|_, _, cx| cx.open_url(RELEASES_PAGE)),
                ),
            );
        v_flex()
            .id("settings")
            .test_support()
            .key_context(SETTINGS)
            .track_focus(&self.focus_handle)
            .on_action(|_: &OpenLogs, _, cx| {
                if let Some(dir) = store::logs_dir() {
                    cx.reveal_path(&dir);
                }
            })
            .gap_2()
            .children(error.map(|problem| {
                let text = match problem {
                    SettingsProblem::Read(detail) => t!("settings.error", detail = detail),
                    SettingsProblem::Write(detail) => t!("settings.error.write", detail = detail),
                };
                let text = SharedString::from(text.to_string());
                div()
                    .id("settings.error")
                    .test_support()
                    .aria_label(text.clone())
                    .text_sm()
                    .text_color(cx.theme().danger)
                    .child(text)
            }))
            .child(rows)
    }
}
