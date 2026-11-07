// SPDX-License-Identifier: GPL-3.0-or-later

//! Puts the settings into effect: theme, font size and table density. The language is set by
//! `i18n`.

use gpui_kit::component::theme::ThemeRegistry;
use gpui_kit::component::{Size, Theme, ThemeMode};
use gpui_kit::{App, Global, Pixels, Window, px};
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

/// Applies the theme, then the font size, which a theme file can set too.
fn apply_appearance(settings: &Settings, window: &mut Window, cx: &mut App) {
    apply_theme(&settings.theme, window, cx);
    let kit = cx.global::<KitFontSize>().0;
    let size = settings.font_size.map_or(kit, |n| px(f32::from(n)));
    Theme::update(cx, |theme| theme.font_size = size);
}

/// What a change of the system appearance does: nothing to an explicit theme.
pub fn appearance_changed(settings: &Settings, window: &mut Window, cx: &mut App) {
    if settings.theme == "system" {
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
