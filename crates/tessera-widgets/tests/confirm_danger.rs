// SPDX-License-Identifier: GPL-3.0-or-later

//! UI tests of the type-to-confirm dialog in a headless GPUI window.

#![allow(clippy::unwrap_used)]

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::test::{TestAppContextExt as _, TestWindowExt as _};
use gpui_kit::{
    AppContext as _, Bounds, Point, TestAppContext, WindowBounds, WindowHandle, WindowOptions,
    base::Root, component::WindowExt as _, px, size,
};
use tessera_widgets::confirm_danger::{self, DangerOptions};

fn open(cx: &mut TestAppContext, prod: bool) -> (WindowHandle<Root>, Rc<Cell<u32>>) {
    let ran = Rc::new(Cell::new(0));
    let handle = cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_kit::component::set_locale("en");
        let bounds = Bounds {
            origin: Point::default(),
            size: size(px(700.), px(500.)),
        };
        let (window, _) = gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            cx,
            |_, cx| cx.new(|_| Empty),
        )
        .unwrap();
        window.downcast::<Root>().unwrap()
    });
    let counter = ran.clone();
    cx.update_window(handle.into(), move |_, window, cx| {
        confirm_danger::open(
            DangerOptions {
                title: "Delete collection".into(),
                consequences: vec!["All documents are lost.".into(), "Aliases break.".into()],
                expected: "books".into(),
                action_label: "Delete".into(),
                prod,
            },
            move |_, _| counter.set(counter.get() + 1),
            window,
            cx,
        );
    })
    .unwrap();
    (handle, ran)
}

struct Empty;

impl gpui_kit::Render for Empty {
    fn render(
        &mut self,
        _: &mut gpui_kit::Window,
        _: &mut gpui_kit::Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        gpui_kit::div()
    }
}

async fn shown(cx: &mut TestAppContext, handle: WindowHandle<Root>, id: &'static str, on: bool) {
    cx.wait_for(handle.into(), Duration::from_millis(1000), move |w, _| {
        w.try_find(id).is_some() == on
    })
    .await;
}

#[gpui_kit::test]
async fn focus_starts_in_the_text_field(cx: &mut TestAppContext) {
    let (handle, _) = open(cx, false);
    shown(cx, handle, "danger.ok", true).await;
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("danger.input").focused(), Some(true));
        assert_ne!(window.find("danger.ok").focused(), Some(true));
    })
    .unwrap();
}

#[gpui_kit::test]
async fn button_disabled_until_exact_match(cx: &mut TestAppContext) {
    let (handle, _) = open(cx, false);
    shown(cx, handle, "danger.ok", true).await;
    for (text, enabled) in [
        ("Books", false),
        ("books ", false),
        (" books", false),
        ("book", false),
        ("books", true),
    ] {
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.press("cmd-a", cx);
            window.press("backspace", cx);
            window.input(text, cx);
            window.render_frame(cx);
            let disabled = window.find("danger.ok").disabled() == Some(true);
            assert_eq!(disabled, !enabled, "input {text:?}");
        })
        .unwrap();
    }
}

#[gpui_kit::test]
async fn enter_with_mismatch_does_nothing(cx: &mut TestAppContext) {
    let (handle, ran) = open(cx, false);
    shown(cx, handle, "danger.ok", true).await;
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.input("book", cx);
        window.press("enter", cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert_eq!(ran.get(), 0);
    shown(cx, handle, "danger.ok", true).await;
}

#[gpui_kit::test]
async fn esc_cancels_without_running(cx: &mut TestAppContext) {
    let (handle, ran) = open(cx, false);
    shown(cx, handle, "danger.ok", true).await;
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.input("books", cx);
        window.press("escape", cx);
    })
    .unwrap();
    shown(cx, handle, "danger.ok", false).await;
    assert_eq!(ran.get(), 0);
}

#[gpui_kit::test]
async fn click_with_match_runs_and_closes(cx: &mut TestAppContext) {
    let (handle, ran) = open(cx, false);
    shown(cx, handle, "danger.ok", true).await;
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.input("books", cx);
        window.render_frame(cx);
        window.click("danger.ok", cx);
    })
    .unwrap();
    shown(cx, handle, "danger.ok", false).await;
    assert_eq!(ran.get(), 1);
}

#[gpui_kit::test]
async fn consequences_are_listed(cx: &mut TestAppContext) {
    let (handle, _) = open(cx, true);
    shown(cx, handle, "danger.ok", true).await;
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("danger.consequence.0").label().unwrap(),
            "All documents are lost."
        );
        assert!(window.try_find("danger.consequence.1").is_some());
        assert!(window.try_find("danger.cancel").is_some());
        window.close_dialog(cx);
    })
    .unwrap();
}
