// SPDX-License-Identifier: GPL-3.0-or-later

//! UI tests of the response body view in a headless GPUI window.

#![allow(clippy::unwrap_used)]

use gpui_kit::{
    AppContext as _, Bounds, Entity, Point, TestAppContext, WindowBounds, WindowHandle,
    WindowOptions, base::Root, px, size,
};
use tessera_widgets::body_view::VIEW_MAX;
use tessera_widgets::{BodyView, Shown};

fn open(cx: &mut TestAppContext) -> (WindowHandle<Root>, Entity<BodyView>) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_kit::component::set_locale("en");
        let bounds = Bounds {
            origin: Point::default(),
            size: size(px(600.), px(400.)),
        };
        let mut view = None;
        let (window, _) = gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            cx,
            |window, cx| {
                let v = cx.new(|cx| BodyView::new(window, cx));
                view = Some(v.clone());
                v
            },
        )
        .unwrap();
        (window.downcast::<Root>().unwrap(), view.unwrap())
    })
}

fn set(
    cx: &mut TestAppContext,
    handle: WindowHandle<Root>,
    view: &Entity<BodyView>,
    text: String,
) -> Shown {
    cx.update_window(handle.into(), |_, window, cx| {
        view.update(cx, |v, cx| v.set_body(text.into(), window, cx));
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| view.read(cx).shown())
}

#[gpui_kit::test]
fn body_view_states(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    assert_eq!(cx.update(|cx| view.read(cx).shown()), Shown::Empty);

    assert_eq!(
        set(cx, handle, &view, r#"{"a": [1]}"#.to_string()),
        Shown::Highlighted
    );
    assert_eq!(
        cx.update(|cx| view.read(cx).text(cx)).unwrap(),
        "{\n  \"a\": [\n    1\n  ]\n}"
    );

    let line = "not json at all\n";
    assert_eq!(
        set(cx, handle, &view, line.repeat(VIEW_MAX / 2 / line.len())),
        Shown::Plain
    );

    let nine = line.repeat((9 << 20) / line.len());
    assert_eq!(
        set(cx, handle, &view, nine.clone()),
        Shown::Chunk { page: 1, pages: 18 }
    );
    cx.update(|cx| view.update(cx, |v, cx| v.copy_all(cx)));
    assert_eq!(cx.read_from_clipboard().and_then(|c| c.text()), Some(nine));

    cx.update_window(handle.into(), |_, window, cx| {
        view.update(cx, |v, cx| v.clear(window, cx));
    })
    .unwrap();
    assert_eq!(cx.update(|cx| view.read(cx).shown()), Shown::Empty);
}

#[gpui_kit::test]
fn paging_reaches_the_whole_body(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    let text = "ä".repeat(VIEW_MAX);
    let Shown::Chunk { pages, .. } = set(cx, handle, &view, text.clone()) else {
        panic!("expected chunks");
    };
    let mut seen = String::new();
    for page in 1..=pages {
        assert_eq!(
            cx.update(|cx| view.read(cx).shown()),
            Shown::Chunk { page, pages }
        );
        seen.push_str(&cx.update(|cx| view.read(cx).text(cx)).unwrap());
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |v, cx| v.next_chunk(window, cx));
        })
        .unwrap();
    }
    assert_eq!(seen, text, "every chunk together is the body");
    assert_eq!(
        cx.update(|cx| view.read(cx).shown()),
        Shown::Chunk { page: pages, pages },
        "stops at the last"
    );
    cx.update_window(handle.into(), |_, window, cx| {
        view.update(cx, |v, cx| v.previous_chunk(window, cx));
    })
    .unwrap();
    assert_eq!(
        cx.update(|cx| view.read(cx).shown()),
        Shown::Chunk {
            page: pages - 1,
            pages
        }
    );
}
