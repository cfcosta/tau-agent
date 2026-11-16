//! [`Doc`]: a layout of text and line breaks, printed to a width. A
//! group is laid out on one line when it fits, and broken at its lines
//! when it does not (Wadler's "prettier printer", without its choice
//! operator).

use std::rc::Rc;

/// Text, the places it may break, and how far broken lines go in.
pub(super) enum Doc {
    Text(String),
    /// Text only when its group breaks: a table's trailing comma.
    Broken(String),
    /// A space on one line, a line break when its group breaks.
    Line,
    /// Nothing on one line, a line break when its group breaks.
    Soft,
    /// Always a line break; every group around one breaks.
    Hard,
    /// Its lines, two spaces further in.
    Indent(Box<Doc>),
    Group {
        broken: bool,
        doc: Box<Doc>,
    },
    /// `first` when its first line fits, else `second`: a call hugging
    /// its last argument, or one argument a line.
    Either {
        first: Box<Doc>,
        second: Box<Doc>,
    },
    Concat(Vec<Doc>),
    /// A call's last argument laid out hugged: measured only up to its
    /// first line break, which it is expected to take.
    Hug(Box<Doc>),
    /// A layout used in more than one place, built once.
    Shared(Rc<Doc>),
}

/// How far an indent goes in.
const INDENT: usize = 2;

pub(super) fn text(text: impl Into<String>) -> Doc {
    Doc::Text(text.into())
}

pub(super) fn indent(doc: Doc) -> Doc {
    Doc::Indent(Box::new(doc))
}

pub(super) fn hug(doc: Doc) -> Doc {
    Doc::Hug(Box::new(doc))
}

pub(super) fn either(first: Doc, second: Doc) -> Doc {
    Doc::Either {
        first: Box::new(first),
        second: Box::new(second),
    }
}

pub(super) fn group(doc: Doc) -> Doc {
    Doc::Group {
        broken: doc.has_hard(),
        doc: Box::new(doc),
    }
}

impl Doc {
    fn has_hard(&self) -> bool {
        match self {
            Doc::Hard => true,
            Doc::Text(_) | Doc::Broken(_) | Doc::Line | Doc::Soft => false,
            Doc::Indent(doc) => doc.has_hard(),
            Doc::Group { broken, .. } => *broken,
            Doc::Either { first, second } => {
                first.has_hard() && second.has_hard()
            }
            Doc::Concat(docs) => docs.iter().any(Doc::has_hard),
            Doc::Shared(doc) => doc.has_hard(),
            Doc::Hug(doc) => doc.has_hard(),
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Flat,
    Break,
}

/// `doc` printed with lines at most `width` columns where it can break,
/// no line ending in spaces.
pub(super) fn render(doc: &Doc, width: usize) -> String {
    let mut out = String::new();
    let mut column = 0;
    let mut stack = vec![(0, Mode::Break, doc)];
    while let Some((depth, mode, doc)) = stack.pop() {
        match doc {
            Doc::Text(text) => {
                out.push_str(text);
                column = match text.rfind('\n') {
                    Some(at) => text[at + 1..].chars().count(),
                    None => column + text.chars().count(),
                };
            }
            Doc::Broken(text) => {
                if mode == Mode::Break {
                    out.push_str(text);
                    column += text.chars().count();
                }
            }
            Doc::Line | Doc::Soft if mode == Mode::Flat => {
                if matches!(doc, Doc::Line) {
                    out.push(' ');
                    column += 1;
                }
            }
            Doc::Line | Doc::Soft | Doc::Hard => {
                while out.ends_with(' ') {
                    out.pop();
                }
                out.push('\n');
                out.extend(std::iter::repeat_n(' ', depth));
                column = depth;
            }
            Doc::Indent(doc) => stack.push((depth + INDENT, mode, doc)),
            Doc::Group { broken, doc } => {
                let flat = !broken
                    && fits(
                        vec![(Mode::Flat, &**doc)],
                        &stack,
                        width.saturating_sub(column),
                    );
                let mode = if flat { Mode::Flat } else { Mode::Break };
                stack.push((depth, mode, doc));
            }
            Doc::Either { first, second } => {
                let left = width.saturating_sub(column);
                // The hugged argument breaks after its first line; the
                // rest is measured on one line.
                let start = match &**first {
                    Doc::Concat(docs) => docs
                        .iter()
                        .rev()
                        .map(|doc| match doc {
                            Doc::Hug(inner) => (Mode::Break, &**inner),
                            doc => (Mode::Flat, doc),
                        })
                        .collect(),
                    doc => vec![(Mode::Flat, doc)],
                };
                let doc = if fits(start, &stack, left) {
                    first
                } else {
                    second
                };
                stack.push((depth, mode, doc));
            }
            Doc::Concat(docs) => {
                stack.extend(docs.iter().rev().map(|doc| (depth, mode, doc)));
            }
            Doc::Shared(doc) => stack.push((depth, mode, doc)),
            Doc::Hug(doc) => stack.push((depth, mode, doc)),
        }
    }
    while out.ends_with(' ') {
        out.pop();
    }
    out
}

/// Whether `stack` (its top last) laid out up to its first line break,
/// and what follows it up to the next, takes at most `left` columns.
fn fits<'d>(
    mut stack: Vec<(Mode, &'d Doc)>,
    rest: &[(usize, Mode, &'d Doc)],
    left: usize,
) -> bool {
    let mut left = left as isize;
    let mut rest = rest.iter().rev();
    loop {
        let Some((mode, doc)) = stack
            .pop()
            .or_else(|| rest.next().map(|&(_, mode, doc)| (mode, doc)))
        else {
            return true;
        };
        match doc {
            Doc::Text(text) => {
                let line = text.split('\n').next().unwrap_or("");
                left -= line.chars().count() as isize;
                if left < 0 {
                    return false;
                }
                if text.contains('\n') {
                    return true;
                }
            }
            Doc::Broken(text) if mode == Mode::Break => {
                left -= text.chars().count() as isize;
            }
            Doc::Broken(_) => {}
            Doc::Line if mode == Mode::Flat => left -= 1,
            Doc::Soft if mode == Mode::Flat => {}
            Doc::Line | Doc::Soft | Doc::Hard => return true,
            Doc::Indent(doc) => stack.push((mode, doc)),
            Doc::Group { broken, doc } => {
                let mode = if *broken { Mode::Break } else { mode };
                stack.push((mode, doc));
            }
            Doc::Either { first, .. } => stack.push((mode, first)),
            Doc::Shared(doc) => stack.push((mode, doc)),
            Doc::Hug(doc) => stack.push((mode, doc)),
            Doc::Concat(docs) => {
                stack.extend(docs.iter().rev().map(|doc| (mode, doc)));
            }
        }
        if left < 0 {
            return false;
        }
    }
}
