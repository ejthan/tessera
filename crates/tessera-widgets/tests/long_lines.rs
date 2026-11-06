// SPDX-License-Identifier: GPL-3.0-or-later

//! A body without a line break stays usable: measured on an M-series Mac in a debug build, one
//! 512 KiB line takes 35 ms per frame and 140 to 230 ms per key press, against 17 ms and 57 to
//! 107 ms for the same text broken every 4 KiB. Layout of a long line is linear, so the view does
//! not break lines.

#![allow(clippy::unwrap_used)]

use std::time::{Duration, Instant};

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, Render, TestAppContext, Window, WindowOptions,
    base::Root,
};
use tessera_widgets::body_view::VIEW_MAX;
use tessera_widgets::{BodyView, Shown};

struct Wrapper(Entity<BodyView>);

impl Render for Wrapper {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.0.clone()
    }
}

/// Several times the measured time, for slow CI machines.
const BOUND: Duration = Duration::from_secs(2);

#[gpui_kit::test]
async fn a_single_line_body_renders_within_a_bound(cx: &mut TestAppContext) {
    let (window, view) = cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_kit::component::set_locale("en");
        let mut view = None;
        let (window, _) = gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
            let v = cx.new(|cx| BodyView::new(window, cx));
            view = Some(v.clone());
            cx.new(|_| Wrapper(v))
        })
        .unwrap();
        (window.downcast::<Root>().unwrap(), view.unwrap())
    });
    // 32 MiB of one line: 64 chunks, of which one is laid out.
    let text: std::sync::Arc<str> = "a".repeat(32 << 20).into();
    let start = Instant::now();
    cx.update_window(window.into(), |_, window, cx| {
        view.update(cx, |v, cx| v.set_body(text, window, cx));
    })
    .unwrap();
    cx.run_until_parked();
    let set = start.elapsed();
    let start = Instant::now();
    cx.update_window(window.into(), |_, window, cx| window.render_frame(cx))
        .unwrap();
    let frame = start.elapsed();
    println!("set_body {set:?}, first frame {frame:?}");
    cx.update(|cx| {
        let v = view.read(cx);
        assert_eq!(v.shown(), Shown::Chunk { page: 1, pages: 64 });
        assert_eq!(v.text(cx).unwrap().len(), VIEW_MAX);
    });
    assert!(set < BOUND && frame < BOUND, "{set:?} {frame:?}");
}
