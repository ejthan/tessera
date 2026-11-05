// SPDX-License-Identifier: GPL-3.0-or-later

//! The detail of one request log entry: request line, headers and bodies, with secrets
//! masked.

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::{ActiveTheme as _, v_flex};
use gpui_kit::{App, IntoElement, ParentElement as _, SharedString, Styled as _, div, prelude::*};
use rust_i18n::t;
use tessera_client::Body;
use tessera_core::log::LogEntry;
use tessera_core::redact;

use super::request_log::path_and_query;

/// The display text of an entry, built once per selection: redacting and pretty-printing a
/// 1 MiB body takes too long for every frame.
pub struct Detail {
    /// Event id of the entry.
    pub id: u64,
    sections: Vec<(SharedString, SharedString)>,
    /// All sections as one text, for assistive technology.
    label: SharedString,
}

impl Detail {
    /// The detail of `entry`. Every body goes through [`redact::json`], the key through
    /// [`redact::key`].
    pub fn new(entry: &LogEntry) -> Self {
        let e = &entry.event;
        // Built as text: `Url` would percent-encode the mask in the query.
        let url = format!(
            "{}{}",
            e.node.origin().ascii_serialization(),
            path_and_query(&e.path, e.query.as_deref())
        );
        let mut sections = vec![(t!("log.detail.request"), format!("{} {url}", e.method))];
        let outcome = match (e.status, &e.error) {
            (Some(status), _) => t!(
                "log.detail.status",
                status = status,
                ms = e.duration.as_millis()
            )
            .to_string(),
            (None, Some(error)) => t!(
                "log.detail.failed",
                error = redact::text_query_params(error)
            )
            .to_string(),
            (None, None) => t!("log.status.failed").to_string(),
        };
        sections.push((t!("log.detail.response"), outcome));
        let headers = std::iter::once(format!(
            "X-TYPESENSE-API-KEY: {}",
            redact::key(&e.key_prefix)
        ))
        .chain(e.request_headers.iter().map(|(k, v)| format!("{k}: {v}")))
        .collect::<Vec<_>>()
        .join("\n");
        sections.push((t!("log.detail.headers"), headers));
        for (heading, body) in [
            (t!("log.detail.request_body"), &e.request_body),
            (t!("log.detail.response_body"), &e.response_body),
        ] {
            if let Some(body) = body {
                sections.push((heading, body_text(body)));
            }
        }
        let label = sections
            .iter()
            .map(|(h, text)| format!("{h}\n{text}"))
            .collect::<Vec<_>>()
            .join("\n\n");
        Self {
            id: e.id,
            sections: sections
                .into_iter()
                .map(|(h, text)| (h.to_string().into(), text.into()))
                .collect(),
            label: label.into(),
        }
    }
}

/// A body, masked and pretty-printed, headed by "showing 1.0 MB of 3.4 MB" when cut.
fn body_text(body: &Body) -> String {
    let text = redact::json(&body.text);
    if body.original_len > body.text.len() {
        let cut = t!(
            "log.detail.cut",
            shown = size(body.text.len() as u64),
            total = size(body.original_len as u64)
        );
        format!("{cut}\n{text}")
    } else {
        text
    }
}

/// `512 B`, `2.0 KB`, `3.4 MB` or `1.5 GB`.
pub fn size(bytes: u64) -> String {
    const KB: f64 = 1024.;
    let b = bytes as f64;
    if b < KB {
        t!("units.bytes", n = bytes).to_string()
    } else if b < KB * KB {
        t!("units.kb", n = format!("{:.1}", b / KB)).to_string()
    } else if b < KB * KB * KB {
        t!("units.mb", n = format!("{:.1}", b / KB / KB)).to_string()
    } else {
        t!("units.gb", n = format!("{:.1}", b / KB / KB / KB)).to_string()
    }
}

/// The detail pane. `None` while the text of a new selection is being built.
pub fn render(detail: Option<&Detail>, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    v_flex()
        .id("log.detail")
        .test_support()
        .when_some(detail, |el, d| el.aria_label(d.label.clone()))
        .size_full()
        .overflow_y_scroll()
        .gap_3()
        .p_3()
        .text_xs()
        .children(
            detail
                .into_iter()
                .flat_map(|d| &d.sections)
                .map(|(h, text)| {
                    v_flex()
                        .gap_1()
                        .child(div().text_color(theme.muted_foreground).child(h.clone()))
                        .child(
                            div()
                                .font_family(theme.mono_font_family.clone())
                                .child(text.clone()),
                        )
                }),
        )
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use tessera_client::{Method, Origin, RequestEvent};
    use uuid::Uuid;

    use super::*;

    #[test]
    fn sizes_have_one_decimal_above_bytes() {
        rust_i18n::set_locale("en");
        assert_eq!(size(512), "512 B");
        assert_eq!(size(2048), "2.0 KB");
        assert_eq!(size(3_565_158), "3.4 MB");
        assert_eq!(size(3 << 29), "1.5 GB");
    }

    #[test]
    fn detail_masks_key_and_bodies() {
        rust_i18n::set_locale("en");
        let entry = LogEntry {
            connection: Uuid::nil(),
            connection_name: "dev".into(),
            event: RequestEvent {
                id: 1,
                started_at: SystemTime::UNIX_EPOCH,
                node: "http://localhost:8108".parse().expect("url"),
                method: Method::POST,
                path: "/keys".into(),
                query: Some("a=1&x-typesense-api-key=sk-query".into()),
                request_headers: vec![("Content-Type".into(), "application/json".into())],
                key_prefix: String::new(),
                request_body: Some(Body {
                    text: r#"{"value":"secret"}"#.into(),
                    original_len: 18,
                }),
                response_body: None,
                status: None,
                duration: Duration::ZERO,
                size: None,
                origin: Origin::User,
                error: Some("connection refused".into()),
            },
        };
        let d = Detail::new(&entry);
        assert!(
            d.label
                .contains("POST http://localhost:8108/keys?a=1&x-typesense-api-key=••••")
        );
        assert!(d.label.contains("X-TYPESENSE-API-KEY: ••••\n"));
        assert!(d.label.contains("Content-Type: application/json"));
        assert!(d.label.contains("connection refused"));
        assert!(!d.label.contains("secret") && !d.label.contains("sk-query"));
    }
}
