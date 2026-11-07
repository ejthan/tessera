// SPDX-License-Identifier: GPL-3.0-or-later

//! The settings dialog: Appearance and Defaults. Plain rows, because the kit's `Settings` picks
//! its pages from a sidebar whose items take no focus, so a keyboard reaches the first page only.

use std::cell::Cell;
use std::ops::RangeInclusive;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::theme::ThemeRegistry;
use gpui_kit::component::{
    ActiveTheme as _, IndexPath, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Focusable as _, IntoElement,
    ParentElement as _, Render, ScrollHandle, SharedString, Styled as _, Subscription, Window, div,
    prelude::*, px,
};
use rust_i18n::t;
use tessera_core::settings::{Density, Language, POLL_CHOICES, Settings};
use tessera_core::transfer::MAX_BATCH;

use crate::state::{AppState, SettingsProblem};

/// Width of the dialog.
const WIDTH: f32 = 560.;
/// Height the dialog keeps free of its rows: its title, its padding and a margin.
const DIALOG_CHROME: f32 = 200.;
/// Width of the control of a row.
const CONTROL_WIDTH: f32 = 200.;
/// The smallest font size; the options follow it up to 20.
const FIRST_FONT_SIZE: u8 = 12;
const FONT_SIZES: usize = 9;

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
            ));
        v_flex()
            .id("settings")
            .test_support()
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
