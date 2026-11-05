// SPDX-License-Identifier: GPL-3.0-or-later

//! UI test of the field type select in a headless GPUI window.

#![allow(clippy::unwrap_used)]

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::WindowHandle;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AppContext as _, Bounds, Entity, Point, TestAppContext, WindowBounds, WindowOptions,
    base::Root, px, size,
};
use tessera_core::schema::FieldType;
use tessera_widgets::{FieldTypeChanged, FieldTypeSelect, field_type_description};

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
    let (handle, _) = open(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let el = window.find("field_type");
        assert!(format!("{el:?}").contains("int32[]"), "{el:?}");
    })
    .unwrap();
}

fn open(cx: &mut TestAppContext) -> (WindowHandle<Root>, Entity<FieldTypeSelect>) {
    let mut select = None;
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
                select = Some(s.clone());
                cx.new(|_| Wrapper(s))
            },
        )
        .unwrap();
        window.downcast::<Root>().unwrap()
    });
    (handle, select.unwrap())
}

#[gpui_kit::test]
async fn picking_a_type_emits_and_set_does_not(cx: &mut TestAppContext) {
    let (handle, select) = open(cx);
    let got: Rc<RefCell<Vec<FieldType>>> = Rc::default();
    let sink = got.clone();
    let _sub = cx.update(|cx| {
        cx.subscribe(&select, move |_, ev: &FieldTypeChanged, _| {
            sink.borrow_mut().push(ev.0.clone())
        })
    });
    cx.update_window(handle.into(), |_, window, cx| {
        select.update(cx, |s, cx| s.set(&FieldType::Bool, window, cx));
        window.render_frame(cx);
    })
    .unwrap();
    assert!(got.borrow().is_empty(), "set emits nothing");
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("field_type", cx);
        window.render_frame(cx);
        window.press("down", cx);
        window.press("down", cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(got.borrow().len(), 1, "one pick, one event");
    assert_ne!(got.borrow()[0], FieldType::Bool);
}

#[gpui_kit::test]
async fn every_type_has_a_description(cx: &mut TestAppContext) {
    cx.update(|_| {
        for lang in ["en", "de"] {
            for ty in FieldType::ALL {
                let d = field_type_description(&ty, lang);
                assert!(!d.is_empty() && d != ty.as_str(), "{lang} {}", ty.as_str());
                assert!(
                    !d.starts_with("field_type."),
                    "{lang} missing key for {}",
                    ty.as_str()
                );
            }
        }
    });
}
