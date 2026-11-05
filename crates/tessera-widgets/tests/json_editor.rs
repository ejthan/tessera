// SPDX-License-Identifier: GPL-3.0-or-later

//! UI tests of the JSON editor in a headless GPUI window.

#![allow(clippy::unwrap_used)]

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AppContext as _, Bounds, Entity, Point, TestAppContext, WindowBounds, WindowHandle,
    WindowOptions, base::Root, px, size,
};
use tessera_widgets::JsonEditor;

fn open(
    cx: &mut TestAppContext,
    text: &'static str,
    read_only: bool,
) -> (WindowHandle<Root>, Entity<JsonEditor>) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_kit::component::set_locale("en");
        let bounds = Bounds {
            origin: Point::default(),
            size: size(px(600.), px(400.)),
        };
        let mut editor = None;
        let (window, _) = gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            cx,
            |window, cx| {
                let e = cx.new(|cx| JsonEditor::new(text, read_only, window, cx));
                editor = Some(e.clone());
                cx.new(|_| Wrapper(e))
            },
        )
        .unwrap();
        (window.downcast::<Root>().unwrap(), editor.unwrap())
    })
}

struct Wrapper(Entity<JsonEditor>);

impl gpui_kit::Render for Wrapper {
    fn render(
        &mut self,
        _: &mut gpui_kit::Window,
        _: &mut gpui_kit::Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        self.0.clone()
    }
}

#[gpui_kit::test]
async fn shows_parse_error_with_line(cx: &mut TestAppContext) {
    let (handle, editor) = open(cx, "{\n  \"a\": 1,\n}", false);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(editor.read(cx).error().unwrap().line, 3);
        assert!(window.find("json.error").label().unwrap().contains('3'));
    })
    .unwrap();
}

#[gpui_kit::test]
async fn valid_json_has_no_error(cx: &mut TestAppContext) {
    let (handle, editor) = open(cx, "{\"a\": 1}", false);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(editor.read(cx).error().is_none());
        assert!(window.try_find("json.error").is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
async fn read_only_ignores_typing(cx: &mut TestAppContext) {
    let (handle, editor) = open(cx, "{}", true);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("json.editor", cx);
        window.input("x", cx);
        assert_eq!(editor.read(cx).text(cx), "{}");
    })
    .unwrap();
}
