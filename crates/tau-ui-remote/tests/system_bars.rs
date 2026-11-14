//! A phone drawn edge to edge: the workspace keeps clear of the system's
//! bars, with a black band behind each, and draws none without them.

use gpui::{Bounds, Pixels, TestAppContext, VisualTestContext, px, size};
use tau_ui_remote::{SystemBars, Workspace, catalog::Catalog};

const WIDTH: f32 = 390.;
const HEIGHT: f32 = 844.;

fn open(
    bars: Option<SystemBars>,
    cx: &mut TestAppContext,
) -> VisualTestContext {
    cx.update(tau_ui_remote::init);
    let window = cx.add_window(|window, cx| {
        let mut workspace =
            Workspace::new("tau", Vec::new(), Catalog::default(), window, cx);
        if let Some(bars) = bars {
            workspace.keep_clear_of_system_bars(move || bars, cx);
        }
        workspace
    });
    let cx = VisualTestContext::from_window(window.into(), cx);
    cx.simulate_resize(size(px(WIDTH), px(HEIGHT)));
    cx.run_until_parked();
    cx
}

fn drawn(selector: &'static str, cx: &mut VisualTestContext) -> Bounds<Pixels> {
    cx.debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is drawn"))
}

#[gpui::test]
fn the_workspace_keeps_clear_of_the_system_bars(cx: &mut TestAppContext) {
    let bars = SystemBars {
        top: 24.,
        bottom: 16.,
    };
    let mut cx = open(Some(bars), cx);
    let top = drawn("system-bar-top", &mut cx);
    assert_eq!((top.top(), top.size.height), (px(0.), px(24.)));
    assert_eq!(top.size.width, px(WIDTH));
    let between = drawn("system-bars-between", &mut cx);
    assert_eq!(
        (between.top(), between.bottom()),
        (px(24.), px(HEIGHT - 16.))
    );
    let bottom = drawn("system-bar-bottom", &mut cx);
    assert_eq!(
        (bottom.top(), bottom.bottom()),
        (px(HEIGHT - 16.), px(HEIGHT))
    );
}

#[gpui::test]
fn without_system_bars_no_band_is_drawn(cx: &mut TestAppContext) {
    let mut cx = open(None, cx);
    assert!(cx.debug_bounds("system-bar-top").is_none());
    assert!(cx.debug_bounds("system-bars-between").is_none());
}
