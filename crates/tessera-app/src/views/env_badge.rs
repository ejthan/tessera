// SPDX-License-Identifier: GPL-3.0-or-later

//! Environment badge and profile colors.

use gpui_kit::component::{ActiveTheme as _, Theme};
use gpui_kit::{App, FontWeight, Hsla, IntoElement, ParentElement as _, Styled as _, div, rgb};
use rust_i18n::t;
use tessera_core::profile::{Environment, ProfileColor};

/// Red of the prod badge, band and frame.
pub const PROD_RED: u32 = 0xDC2626;

/// The profile colors with the key suffix of their names (`connect.color.{name}`).
pub const COLORS: [(ProfileColor, &str); 6] = [
    (ProfileColor::Gray, "gray"),
    (ProfileColor::Blue, "blue"),
    (ProfileColor::Teal, "teal"),
    (ProfileColor::Violet, "violet"),
    (ProfileColor::Amber, "amber"),
    (ProfileColor::Red, "red"),
];

/// The accent color of a profile, from the design canvas.
pub fn color(color: ProfileColor, _theme: &Theme) -> Hsla {
    rgb(match color {
        ProfileColor::Gray => 0x71717A,
        ProfileColor::Blue => 0x2563EB,
        ProfileColor::Teal => 0x0D9488,
        ProfileColor::Violet => 0x7C3AED,
        ProfileColor::Amber => 0xB45309,
        ProfileColor::Red => PROD_RED,
    })
    .into()
}

/// Display text of an environment: the built-in tags are translated, custom ones shown as typed.
pub fn label(env: &Environment) -> String {
    match env {
        Environment::Local => t!("env.local").to_string(),
        Environment::Dev => t!("env.dev").to_string(),
        Environment::Staging => t!("env.staging").to_string(),
        Environment::Prod => t!("env.prod").to_string(),
        Environment::Custom(name) => name.to_uppercase(),
    }
}

/// The environment badge: prod red and filled, other tags subdued.
pub fn render(env: &Environment, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let badge = div()
        .flex_none()
        .px_1()
        .rounded_sm()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .child(label(env));
    if env.is_prod() {
        badge.bg(rgb(PROD_RED)).text_color(gpui_kit::white())
    } else {
        badge.bg(theme.muted).text_color(theme.muted_foreground)
    }
}

/// A small dot in the profile color.
pub fn dot(color: ProfileColor, cx: &App) -> impl IntoElement {
    div()
        .flex_none()
        .size_2()
        .rounded_full()
        .bg(self::color(color, cx.theme()))
}
