//! The views of plugins' tree widgets (ADR 0045), kept by the window
//! from one draw to the next.
//!
//! - A terminal: the tree carries what was written; the window keeps a
//!   [`TerminalView`] per terminal, for its scrollback and selection,
//!   fed what the tree gained since it last drew, and frozen once the
//!   program ends.
//! - A ticker: the tree carries when it started and stopped, or when it
//!   is due; the window draws what it reads now, and keeps a timer that
//!   draws it again each second while it still changes and is shown.

use std::{cell::RefCell, collections::HashMap, rc::Rc};

use gpui::{
    AnyElement,
    App,
    AppContext as _,
    Entity,
    IntoElement,
    ParentElement as _,
    rems,
};
use tau_terminal::{Size, TerminalView, ViewOptions};
use tau_ui_kit::{
    components::mono,
    theme::{MONO, Theme, Type, theme},
    ticker::now,
};
use tau_ui_plugin::{
    Handle,
    tree::{TerminalSpec, TickerSpec, WidgetSpec},
};

/// One terminal's screen, and what it was fed.
struct Screen {
    view: Entity<TerminalView>,
    fed: Vec<u8>,
    size: (u16, u16),
}

/// Every tree widget's view in one window, by plugin and key.
#[derive(Default)]
pub struct TreeWidgets {
    screens: HashMap<String, Screen>,
    tickers: HashMap<String, Ticking>,
    pictures: HashMap<String, Picture>,
    /// Each area that takes the keys: its focus, and the focus number it
    /// last took them for.
    areas: HashMap<String, (gpui::FocusHandle, Option<u64>)>,
}

/// A tree's image, as far as the window has it.
#[derive(Clone)]
enum Picture {
    /// Asked for at an instant; its type, to decode it with.
    Waiting(String, std::time::Instant),
    Ready(std::sync::Arc<gpui::Image>),
    Failed(String),
}

/// How long an image asked for waits before it is asked for again.
const RETRY: std::time::Duration = std::time::Duration::from_secs(10);

/// The format gpui decodes an image of type `mime` as.
fn format_of(mime: &str) -> Option<gpui::ImageFormat> {
    use gpui::ImageFormat;
    Some(match mime {
        "image/png" => ImageFormat::Png,
        "image/jpeg" => ImageFormat::Jpeg,
        "image/gif" => ImageFormat::Gif,
        "image/webp" => ImageFormat::Webp,
        "image/bmp" => ImageFormat::Bmp,
        "image/svg+xml" => ImageFormat::Svg,
        _ => return None,
    })
}

impl TreeWidgets {
    /// Keeps what `plugin`'s host half answered for its picture `key`:
    /// `{ "data": base64 }`, or `{ "error": why }`. An answer nothing
    /// waits for is dropped.
    pub fn fetched(
        &mut self,
        plugin: &str,
        key: &str,
        reply: &serde_json::Value,
    ) {
        use base64::Engine as _;

        let id = format!("{plugin}\u{0}{key}");
        let Some(Picture::Waiting(mime, _)) = self.pictures.get(&id).cloned()
        else {
            return;
        };
        let data = reply.get("data").and_then(serde_json::Value::as_str);
        let picture = match (data, format_of(&mime)) {
            (Some(data), Some(format)) => {
                match base64::engine::general_purpose::STANDARD.decode(data) {
                    Ok(bytes) => Picture::Ready(std::sync::Arc::new(
                        gpui::Image::from_bytes(format, bytes),
                    )),
                    Err(error) => Picture::Failed(error.to_string()),
                }
            }
            (Some(_), None) => Picture::Failed(format!("cannot draw {mime}")),
            (None, _) => Picture::Failed(
                reply
                    .get("error")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("nothing came")
                    .to_owned(),
            ),
        };
        self.pictures.insert(id, picture);
    }
}

/// The area of `spec` for `handle`'s plugin: its keys go to the plugin
/// while it has them, its fields let `through` keys come to it, a click
/// outside its fields gives it the keys, and so does a new focus number.
fn area(
    widgets: &Rc<RefCell<TreeWidgets>>,
    handle: &Handle,
    spec: tau_ui_plugin::tree::KeysSpec,
    cx: &mut App,
) -> AnyElement {
    use gpui::{InteractiveElement as _, ParentElement as _, Styled as _};

    let id = format!("{}\u{0}{}", handle.plugin(), spec.key);
    let (focus, refocus) = {
        let mut all = widgets.borrow_mut();
        let (focus, last) = all
            .areas
            .entry(id)
            .or_insert_with(|| (cx.focus_handle(), None));
        let refocus = spec.focus.is_some() && spec.focus != *last;
        *last = spec.focus;
        (focus.clone(), refocus)
    };
    if refocus {
        handle.focus(&focus, cx);
    }
    let (keys, through, on_key) = (spec.keys, spec.through, spec.on_key);
    let (pressed, clicked) = (focus.clone(), focus.clone());
    gpui::div()
        .track_focus(&focus)
        .flex()
        .flex_col()
        .on_key_down(move |event: &gpui::KeyDownEvent, window, cx| {
            let Some(on_key) = &on_key else {
                return;
            };
            let stroke = &event.keystroke;
            let held = stroke.modifiers.control
                || stroke.modifiers.alt
                || stroke.modifiers.platform
                || stroke.modifiers.function;
            let key = stroke.key.as_str();
            if held {
                return;
            }
            if pressed.is_focused(window) {
                if keys.iter().any(|k| k == key) {
                    cx.stop_propagation();
                    on_key(key, false, cx);
                }
            } else if pressed.contains_focused(window, cx)
                && through.iter().any(|k| k == key)
            {
                cx.stop_propagation();
                on_key(key, true, cx);
            }
        })
        .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
            // A field's own click keeps the keys; anywhere else in the
            // area takes them.
            let in_field = clicked.contains_focused(window, cx)
                && !clicked.is_focused(window);
            if !in_field {
                window.focus(&clicked, cx);
            }
        })
        .children(spec.children)
        .into_any_element()
}

/// The picture of `spec` for `handle`'s plugin, drawn: asked of the host
/// the first time, and again while no answer came for a while.
fn picture(
    widgets: &Rc<RefCell<TreeWidgets>>,
    handle: &Handle,
    spec: tau_ui_plugin::tree::ImageSpec,
    cx: &mut App,
) -> AnyElement {
    use gpui::{Styled as _, StyledImage as _, prelude::FluentBuilder as _};
    use tau_ui_kit::theme::{Design as _, radius, sp};

    let id = format!("{}\u{0}{}", handle.plugin(), spec.key);
    let kept = widgets.borrow().pictures.get(&id).cloned();
    let picture = match kept {
        Some(Picture::Waiting(_, at)) if at.elapsed() >= RETRY => None,
        kept => kept,
    };
    let picture = picture.unwrap_or_else(|| {
        let waiting =
            Picture::Waiting(spec.mime.clone(), std::time::Instant::now());
        widgets.borrow_mut().pictures.insert(id, waiting.clone());
        handle.fetch(spec.key.clone(), spec.fetch.clone(), cx);
        waiting
    });
    let t = theme(cx).clone();
    let frame = gpui::div()
        .w_full()
        .flex()
        .items_center()
        .justify_center()
        .when_some(spec.height, |frame, height| {
            frame.h(rems(height.clamp(2., 80.)))
        });
    match picture {
        Picture::Ready(image) => frame
            .child(
                gpui::img(image)
                    .size_full()
                    .object_fit(gpui::ObjectFit::Contain),
            )
            .into_any_element(),
        Picture::Waiting(..) => frame
            .min_h(rems(4.))
            .rounded(radius::CARD)
            .bg(t.raised)
            .child(mono("Loading the image…", Type::CAPTION, t.dim))
            .into_any_element(),
        Picture::Failed(error) => frame
            .min_h(rems(4.))
            .rounded(radius::CARD)
            .bg(t.raised)
            .p(sp(3.))
            .child(
                gpui::div()
                    .typeset(Type::CAPTION)
                    .text_color(t.muted)
                    .child(format!("The image could not be loaded: {error}")),
            )
            .into_any_element(),
    }
}

/// A ticker's timer, and when the ticker was last drawn: the timer stops
/// once it has not been for a while.
struct Ticking {
    drawn: Rc<std::cell::Cell<std::time::Instant>>,
    _timer: gpui::Task<()>,
}

/// How long a ticker that is no longer drawn keeps its timer.
const SHOWN_FOR: std::time::Duration = std::time::Duration::from_secs(2);

/// `spec`'s widget for `plugin`, drawn; `handle` draws the window again.
pub fn widget(
    widgets: &Rc<RefCell<TreeWidgets>>,
    handle: &Handle,
    spec: WidgetSpec,
    cx: &mut App,
) -> AnyElement {
    match spec {
        WidgetSpec::Terminal(spec) => {
            screen(widgets, handle.plugin(), spec, cx)
        }
        WidgetSpec::Ticker(spec) => {
            let text = tick(widgets, handle, spec, cx);
            mono(text, Type::CAPTION, theme(cx).text_soft).into_any_element()
        }
        WidgetSpec::Image(spec) => picture(widgets, handle, spec, cx),
        WidgetSpec::Keys(spec) => area(widgets, handle, spec, cx),
    }
}

/// What the ticker of `spec` reads now, with a timer drawing the window
/// again each second while it still changes; none once it stops.
fn tick(
    widgets: &Rc<RefCell<TreeWidgets>>,
    handle: &Handle,
    spec: TickerSpec,
    cx: &mut App,
) -> String {
    let id = format!("{}\u{0}{}", handle.plugin(), spec.key);
    let (text, changes) = spec.count.read(now());
    let mut all = widgets.borrow_mut();
    if !changes {
        all.tickers.remove(&id);
        return text;
    }
    if let Some(ticking) = all.tickers.get(&id) {
        ticking.drawn.set(std::time::Instant::now());
        return text;
    }
    let drawn = Rc::new(std::cell::Cell::new(std::time::Instant::now()));
    let (shown, handle) = (drawn.clone(), handle.clone());
    let timer = cx.spawn(async move |cx| {
        loop {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(1))
                .await;
            if shown.get().elapsed() > SHOWN_FOR {
                return;
            }
            cx.update(|cx| handle.refresh(cx));
        }
    });
    all.tickers.insert(
        id,
        Ticking {
            drawn,
            _timer: timer,
        },
    );
    text
}

fn options(cols: u16, rows: u16, shown: Option<u16>, t: &Theme) -> ViewOptions {
    ViewOptions {
        size: Size { cols, rows },
        font_family: MONO.into(),
        font_size: rems(t.term.text.size / 16.),
        line_height: t.term.leading,
        palette: t.term.palette,
        visible_rows: shown.map(usize::from),
        ..ViewOptions::default()
    }
}

/// A screen showing `bytes` on a terminal of `options`' size: live, to
/// be fed more, or frozen.
fn made(
    bytes: &[u8],
    live: bool,
    options: ViewOptions,
    cx: &mut App,
) -> Option<Entity<TerminalView>> {
    let mut failed = false;
    let view = cx.new(|cx| {
        let view = if live {
            TerminalView::new(options.clone(), cx).and_then(|mut view| {
                view.write(bytes, cx)?;
                Ok(view)
            })
        } else {
            TerminalView::replay(bytes, options.clone(), cx)
        };
        view.unwrap_or_else(|_| {
            failed = true;
            TerminalView::frozen(Vec::new(), options.size.cols, options, cx)
        })
    });
    (!failed).then_some(view)
}

/// The screen of `spec`'s terminal for `plugin`, drawn: made the first
/// time, then fed what was written since, or made again from the start
/// when what was written is not what it was fed and more.
fn screen(
    terminals: &Rc<RefCell<TreeWidgets>>,
    plugin: &str,
    spec: TerminalSpec,
    cx: &mut App,
) -> AnyElement {
    match view(terminals, plugin, spec, cx) {
        Some(view) => view.into_any_element(),
        None => gpui::div()
            .child("This terminal could not be drawn.")
            .into_any_element(),
    }
}

/// [`screen`]'s view, or none when no terminal could be made.
fn view(
    terminals: &Rc<RefCell<TreeWidgets>>,
    plugin: &str,
    spec: TerminalSpec,
    cx: &mut App,
) -> Option<Entity<TerminalView>> {
    let id = format!("{plugin}\u{0}{}", spec.key);
    let size = (spec.cols, spec.rows);
    let kept =
        terminals.borrow().screens.get(&id).map(|screen| {
            (screen.view.clone(), screen.fed.clone(), screen.size)
        });
    let view = match kept {
        Some((view, fed, was))
            if was == size && spec.bytes.starts_with(&fed) =>
        {
            let more = spec.bytes[fed.len()..].to_vec();
            let fed_all = view.update(cx, |view, cx| {
                if view.is_frozen() {
                    return more.is_empty();
                }
                if !more.is_empty() && view.write(&more, cx).is_err() {
                    return false;
                }
                if !spec.live {
                    let _ = view.freeze(cx);
                }
                true
            });
            if fed_all {
                if let Some(screen) =
                    terminals.borrow_mut().screens.get_mut(&id)
                {
                    screen.fed = spec.bytes.clone();
                }
                Some(view)
            } else {
                None
            }
        }
        _ => None,
    };
    let view = match view {
        Some(view) => view,
        None => {
            let t = theme(cx).clone();
            let view = made(
                &spec.bytes,
                spec.live,
                options(spec.cols, spec.rows, spec.shown, &t),
                cx,
            )?;
            terminals.borrow_mut().screens.insert(
                id,
                Screen {
                    view: view.clone(),
                    fed: spec.bytes,
                    size,
                },
            );
            view
        }
    };
    Some(view)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(bytes: &str, live: bool) -> TerminalSpec {
        TerminalSpec {
            key: "out.terminal".into(),
            bytes: bytes.as_bytes().to_vec(),
            cols: 20,
            rows: 4,
            live,
            shown: None,
        }
    }

    /// A ticker that still changes keeps one timer for its key, and one
    /// that stopped keeps none; each reads what its count says.
    #[gpui::test]
    fn a_ticker_keeps_a_timer_while_it_changes(cx: &mut gpui::TestAppContext) {
        use tau_ui_kit::ticker::Count;

        let widgets = Rc::new(RefCell::new(TreeWidgets::default()));
        let handle = Handle::new("p", Rc::new(|_, _, _| {}));
        let spec = |count| TickerSpec {
            key: "load.ticker".into(),
            count,
        };
        let going = Count::Since {
            since: now() - 83_000,
            until: None,
        };
        let read = cx.update(|cx| tick(&widgets, &handle, spec(going), cx));
        assert_eq!(read, "1:23");
        cx.update(|cx| tick(&widgets, &handle, spec(going), cx));
        assert_eq!(widgets.borrow().tickers.len(), 1);
        let stopped = Count::Since {
            since: 1_000,
            until: Some(84_999),
        };
        let read = cx.update(|cx| tick(&widgets, &handle, spec(stopped), cx));
        assert_eq!(read, "1:23");
        assert!(widgets.borrow().tickers.is_empty());
    }

    /// A terminal is fed what it gained while it runs, frozen when it
    /// ends, and made again when what was written is not what it was fed
    /// and more.
    #[gpui::test]
    fn a_terminal_follows_what_was_written(cx: &mut gpui::TestAppContext) {
        cx.update(tau_ui_kit::init);
        let terminals = Rc::new(RefCell::new(TreeWidgets::default()));
        let kept = || {
            terminals
                .borrow()
                .screens
                .values()
                .next()
                .unwrap()
                .view
                .clone()
        };
        cx.update(|cx| drop(view(&terminals, "p", spec("one\r\n", true), cx)));
        let first = kept();
        cx.update(|cx| {
            drop(view(&terminals, "p", spec("one\r\ntwo\r\n", true), cx))
        });
        assert_eq!(kept(), first, "fed, not made again");
        assert!(!first.read_with(cx, |view, _| view.is_frozen()));
        cx.update(|cx| {
            drop(view(&terminals, "p", spec("one\r\ntwo\r\n", false), cx))
        });
        assert!(first.read_with(cx, |view, _| view.is_frozen()));
        cx.update(|cx| {
            drop(view(&terminals, "p", spec("other\r\n", false), cx))
        });
        assert_ne!(kept(), first, "made again");
        assert_eq!(terminals.borrow().screens.len(), 1);
        // The window that kept them closes.
        drop(first);
        terminals.borrow_mut().screens.clear();
        cx.run_until_parked();
    }
}
