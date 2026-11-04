// SPDX-License-Identifier: GPL-3.0-or-later

//! UI tests in headless GPUI windows.

use gpui_kit::{
    AppContext as _, Bounds, Point, TestAppContext, WindowBounds, WindowHandle, WindowOptions,
    base::Root, px, size,
};
use tessera_app::views::root::RootView;

fn open(cx: &mut TestAppContext) -> WindowHandle<Root> {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_kit::component::set_locale("en");
        tessera_app::actions::bind_keys(cx);
        let bounds = Bounds {
            origin: Point::default(),
            size: size(px(1200.), px(800.)),
        };
        let (window, _) = gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            cx,
            |window, cx| cx.new(|cx| RootView::new(window, cx)),
        )
        .expect("open test window");
        window.downcast::<Root>().expect("Base Root")
    })
}

#[gpui_kit::test]
fn shell_shows_not_connected(cx: &mut TestAppContext) {
    use gpui_kit::test::TestWindowExt;
    let handle = open(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("status.text").label(), Some("Not connected"));
        assert_eq!(
            window.find("titlebar.connection").label(),
            Some("No connection")
        );
        assert!(window.find("sidebar").visible());
    })
    .unwrap();
}
