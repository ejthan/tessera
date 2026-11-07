// SPDX-License-Identifier: GPL-3.0-or-later

//! Puts the settings into effect: theme, font size and table density. The language is set by
//! `i18n`.

use gpui_kit::component::table::{TableDelegate, TableState};
use gpui_kit::component::theme::ThemeRegistry;
use gpui_kit::component::{Size, Theme, ThemeColor, ThemeMode};
use gpui_kit::{App, Context, Global, Hsla, Pixels, Window, px, rgb};
use tessera_core::settings::{Density, Settings};

/// The kit's font size, read before the first change.
struct KitFontSize(Pixels);

impl Global for KitFontSize {}

/// The settings in effect, for views without the `AppState`, such as a table's delegate.
struct Current(Settings);

impl Global for Current {}

/// The settings in effect.
pub fn current(cx: &App) -> Settings {
    cx.try_global::<Current>()
        .map_or_else(Settings::default, |c| c.0.clone())
}

/// Keeps the headers of this table in the language in use: the kit caches its column titles.
/// A refresh takes the widths from the delegate again.
pub fn follow_language<D: TableDelegate>(cx: &mut Context<TableState<D>>) {
    let mut shown = rust_i18n::locale().to_string();
    cx.observe_global::<Current>(move |table, cx| {
        let locale = rust_i18n::locale().to_string();
        if locale != shown {
            shown = locale;
            table.refresh(cx);
            cx.notify();
        }
    })
    .detach();
}

/// The size of a data table, `Size::XSmall` when compact.
pub fn table_size(cx: &App) -> Size {
    match cx.try_global::<Current>() {
        Some(Current(Settings {
            density: Density::Compact,
            ..
        })) => Size::XSmall,
        _ => Size::default(),
    }
}

/// Applies the theme: `system`, `light`, `dark` or a name in the registry. A name that is not
/// there, such as a removed theme, follows the system.
fn apply_theme(name: &str, window: &mut Window, cx: &mut App) {
    let registry = ThemeRegistry::global(cx);
    let (light, dark) = (
        registry.default_light_theme().clone(),
        registry.default_dark_theme().clone(),
    );
    let named = registry.themes().get(name).cloned();
    let theme = Theme::global_mut(cx);
    theme.light_theme = light;
    theme.dark_theme = dark;
    match (name, named) {
        ("light", _) => Theme::change(ThemeMode::Light, Some(window), cx),
        ("dark", _) => Theme::change(ThemeMode::Dark, Some(window), cx),
        ("system", _) | (_, None) => Theme::sync_system_appearance(Some(window), cx),
        (_, Some(config)) => {
            let theme = Theme::global_mut(cx);
            if config.mode.is_dark() {
                theme.dark_theme = config.clone();
            } else {
                theme.light_theme = config.clone();
            }
            Theme::change(config.mode, Some(window), cx);
        }
    }
}

/// Raises the colors of the kit that miss WCAG AA, for text at 4.5:1 and for the focus ring
/// at 3:1, in the built-in themes; a theme of the registry keeps what its author set.
/// `ui_a11y` measures each pair.
pub fn aa_colors(colors: &mut ThemeColor, dark: bool) {
    let hex = |value: u32| Hsla::from(rgb(value));
    if dark {
        colors.danger_hover = hex(0xfca5a5);
        colors.danger_active = hex(0xf56565);
        colors.danger_foreground = hex(0x450a0a);
        colors.success_hover = hex(0x86efac);
        colors.success_active = hex(0x22c55e);
        colors.success_foreground = hex(0x052e16);
        colors.warning_hover = hex(0xfde047);
        colors.warning_active = hex(0xeab308);
        colors.warning_foreground = hex(0x422006);
    } else {
        colors.muted_foreground = hex(0x525252);
        colors.danger = hex(0xb91c1c);
        colors.danger_hover = hex(0x991b1b);
        colors.danger_active = hex(0x7f1d1d);
        colors.danger_foreground = hex(0xffffff);
        colors.success = hex(0x15803d);
        colors.success_hover = hex(0x166534);
        colors.success_active = hex(0x14532d);
        colors.success_foreground = hex(0xffffff);
        colors.warning = hex(0xa16207);
        colors.warning_hover = hex(0x854d0e);
        colors.warning_active = hex(0x713f12);
        colors.warning_foreground = hex(0xffffff);
        colors.ring = hex(0x737373);
    }
}

/// Applies the theme, then the font size, which a theme file can set too, and the contrast
/// fixes. They need an update of their own: loading a theme replaces the colors.
fn apply_appearance(settings: &Settings, window: &mut Window, cx: &mut App) {
    apply_theme(&settings.theme, window, cx);
    let kit = cx.global::<KitFontSize>().0;
    let size = settings.font_size.map_or(kit, |n| px(n as f32));
    let built_in = built_in(&settings.theme, cx);
    Theme::update(cx, |theme| {
        theme.font_size = size;
        if built_in {
            let dark = theme.mode.is_dark();
            aa_colors(&mut theme.colors, dark);
        }
    });
}

/// Whether `name` ends on one of the kit's two default themes.
fn built_in(name: &str, cx: &App) -> bool {
    matches!(name, "system" | "light" | "dark")
        || ThemeRegistry::global(cx)
            .themes()
            .get(name)
            .is_none_or(|config| config.is_default)
}

/// Whether `name` leaves the mode to the system: `system`, or a theme that was removed.
fn follows_system(name: &str, cx: &App) -> bool {
    let known = ThemeRegistry::global(cx).themes().contains_key(name);
    name == "system" || !(matches!(name, "light" | "dark") || known)
}

/// What a change of the system appearance does: nothing to an explicit theme.
pub fn appearance_changed(settings: &Settings, window: &mut Window, cx: &mut App) {
    if follows_system(&settings.theme, cx) {
        apply_appearance(settings, window, cx);
    }
}

/// Puts `settings` into effect; first in `RootView::new`, then after each change.
pub fn apply(settings: &Settings, window: &mut Window, cx: &mut App) {
    if !cx.has_global::<KitFontSize>() {
        cx.set_global(KitFontSize(Theme::global(cx).font_size));
    }
    apply_appearance(settings, window, cx);
    cx.set_global(Current(settings.clone()));
    cx.refresh_windows();
}
