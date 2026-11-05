// SPDX-License-Identifier: GPL-3.0-or-later

//! The inline result of the last Test.

use std::time::{Duration, Instant};

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::{ActiveTheme as _, v_flex};
use gpui_kit::{App, IntoElement, ParentElement as _, SharedString, Styled as _, div, prelude::*};
use rust_i18n::t;
use secrecy::{ExposeSecret as _, SecretString};
use tessera_client::KeyAccess;
use tessera_core::profile::Profile;

use crate::connection::ServerInfo;
use crate::views::overview;

/// What the last Test found, and for which values.
pub struct Outcome {
    pub at: Instant,
    pub elapsed: Duration,
    /// The tested profile and typed key.
    pub profile: Profile,
    pub typed: Option<SecretString>,
    pub result: Result<(ServerInfo, KeyAccess), SharedString>,
}

impl Outcome {
    /// Whether the test ran for exactly these values.
    pub fn matches(&self, profile: &Profile, typed: &Option<SecretString>) -> bool {
        let same_key = match (&self.typed, typed) {
            (Some(a), Some(b)) => a.expose_secret() == b.expose_secret(),
            (None, None) => true,
            _ => false,
        };
        self.profile == *profile && same_key
    }
}

/// The result lines; a stale result is muted and says the values changed.
pub fn render(outcome: &Outcome, stale: bool, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let (mut lines, color) = match &outcome.result {
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
            let color = if info.healthy {
                theme.success
            } else {
                theme.warning
            };
            (vec![health.to_string(), access.to_string()], color)
        }
        Err(e) => (vec![e.to_string()], theme.danger),
    };
    let secs = outcome.at.elapsed().as_secs();
    let ago = match secs {
        0..5 => t!("connect.result.just_now"),
        5..60 => t!("connect.result.seconds_ago", n = secs),
        _ => t!("connect.result.minutes_ago", n = secs / 60),
    };
    lines.push(
        t!(
            "connect.result.tested",
            ago = ago,
            ms = outcome.elapsed.as_millis()
        )
        .to_string(),
    );
    if stale {
        lines.push(t!("connect.result.stale").to_string());
    }
    let label = SharedString::from(lines.join("\n"));
    v_flex()
        .id("connect.result")
        .test_support()
        .aria_label(label)
        .text_sm()
        .text_color(if stale { theme.muted_foreground } else { color })
        .children(lines.into_iter().map(|l| div().child(l)))
}
