//! The screens of plugins' tree terminals (ADR 0045). The tree carries
//! what was written; the window keeps a [`TerminalView`] per terminal,
//! for its scrollback and selection, fed what the tree gained since it
//! last drew, and frozen once the program ends.

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
use tau_ui_kit::theme::{MONO, Theme, theme};
use tau_ui_plugin::tree::TerminalSpec;

/// One terminal's screen, and what it was fed.
struct Screen {
    view: Entity<TerminalView>,
    fed: Vec<u8>,
    size: (u16, u16),
}

/// Every tree terminal's screen in one window, by plugin and key.
#[derive(Default)]
pub struct TreeTerminals {
    screens: HashMap<String, Screen>,
}

fn options(cols: u16, rows: u16, t: &Theme) -> ViewOptions {
    ViewOptions {
        size: Size { cols, rows },
        font_family: MONO.into(),
        font_size: rems(t.term.text.size / 16.),
        line_height: t.term.leading,
        palette: t.term.palette,
        visible_rows: Some(usize::from(rows).min(t.term.rows)),
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
pub fn screen(
    terminals: &Rc<RefCell<TreeTerminals>>,
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
    terminals: &Rc<RefCell<TreeTerminals>>,
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
                options(spec.cols, spec.rows, &t),
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
        }
    }

    /// A terminal is fed what it gained while it runs, frozen when it
    /// ends, and made again when what was written is not what it was fed
    /// and more.
    #[gpui::test]
    fn a_terminal_follows_what_was_written(cx: &mut gpui::TestAppContext) {
        cx.update(tau_ui_kit::init);
        let terminals = Rc::new(RefCell::new(TreeTerminals::default()));
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
