// SPDX-License-Identifier: GPL-3.0-or-later

//! UI test of the field type select in a headless GPUI window.

#![allow(clippy::unwrap_used)]

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AppContext as _, Bounds, Entity, Point, TestAppContext, WindowBounds, WindowOptions,
    base::Root, px, size,
};
use tessera_core::schema::FieldType;
use tessera_widgets::FieldTypeSelect;

struct Wrapper(Entity<FieldTypeSelect>);

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
async fn shows_the_selected_type(cx: &mut TestAppContext) {
    let handle = cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_kit::component::set_locale("en");
        let bounds = Bounds {
            origin: Point::default(),
            size: size(px(400.), px(200.)),
        };
        let (window, _) = gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            cx,
            |window, cx| {
                let s = cx.new(|cx| FieldTypeSelect::new(&FieldType::Int32Array, window, cx));
                cx.new(|_| Wrapper(s))
            },
        )
        .unwrap();
        window.downcast::<Root>().unwrap()
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let el = window.find("field_type");
        assert!(format!("{el:?}").contains("int32[]"), "{el:?}");
    })
    .unwrap();
}
