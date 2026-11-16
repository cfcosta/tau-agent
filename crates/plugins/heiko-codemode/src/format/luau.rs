//! Luau laid out from its tree-sitter tree: one statement a line, each
//! block two spaces further in, tables and argument lists on one line
//! when they fit and one item a line when they do not. Only whitespace
//! changes, and a table's trailing comma (written when the table breaks,
//! left out when it does not): every other token and comment is kept, in
//! order, as it was, and a blank line between statements stays one.

use std::{cell::RefCell, collections::HashMap, rc::Rc};

use tree_sitter::{Node, Parser};

use super::doc::{self, Doc, group, indent, text};

/// `code` laid out to `width` columns, or `None` when it does not parse.
pub(super) fn format(code: &str, width: usize) -> Option<String> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_luau::LANGUAGE.into())
        .ok()?;
    let tree = parser.parse(code, None)?;
    let root = tree.root_node();
    if root.has_error() {
        return None;
    }
    let layout = Layout {
        code,
        built: RefCell::new(HashMap::new()),
    };
    Some(doc::render(&layout.statements(root), width))
}

/// A node written as it is: a token, a string (interpolations and all),
/// a comment, or a node with text of its own between its children (an
/// attribute's `@`), which laying out the children would lose.
fn atomic(node: Node, code: &str) -> bool {
    if node.child_count() == 0
        || matches!(node.kind(), "string" | "comment" | "hash_bang_line")
    {
        return true;
    }
    let mut at = node.start_byte();
    for child in children(node) {
        if !code[at..child.start_byte()].trim().is_empty() {
            return true;
        }
        at = child.end_byte();
    }
    !code[at..node.end_byte()].trim().is_empty()
}

fn first_leaf<'t>(mut node: Node<'t>, code: &str) -> Node<'t> {
    while !atomic(node, code) {
        match node.child(0) {
            Some(child) => node = child,
            None => break,
        }
    }
    node
}

fn last_leaf<'t>(mut node: Node<'t>, code: &str) -> Node<'t> {
    while !atomic(node, code) {
        match node.child(node.child_count().saturating_sub(1)) {
            Some(child) => node = child,
            None => break,
        }
    }
    node
}

/// Whether `node`'s last child is a block, as an `else` clause's is:
/// what follows it goes on a new line.
fn ends_in_block(node: Node) -> bool {
    let count = node.child_count();
    count > 0
        && node
            .child(count - 1)
            .is_some_and(|last| last.kind() == "block" || ends_in_block(last))
}

/// The kind of the node a token belongs to.
fn owner<'t>(node: Node<'t>) -> &'t str {
    node.parent().map(|parent| parent.kind()).unwrap_or("")
}

fn children(node: Node) -> Vec<Node> {
    let mut cursor = node.walk();
    node.children(&mut cursor).collect()
}

/// A comment that runs to the end of its line: whatever follows it goes
/// on the next.
fn line_comment(node: Node, code: &str) -> bool {
    node.kind() == "comment" && !long_bracket(&code[node.byte_range()][2..])
}

/// Whether `text` opens a long bracket: `[[`, `[=[`, `[==[`...
fn long_bracket(text: &str) -> bool {
    text.strip_prefix('[')
        .map(|rest| rest.trim_start_matches('=').starts_with('['))
        .unwrap_or(false)
}

struct Layout<'a> {
    code: &'a str,
    /// Each node's layout, by node id: a hugged call lays its
    /// arguments out two ways, and both share them.
    built: RefCell<HashMap<usize, Rc<Doc>>>,
}

impl Layout<'_> {
    fn source(&self, node: Node) -> &str {
        &self.code[node.byte_range()]
    }

    fn node(&self, node: Node) -> Doc {
        if let Some(doc) = self.built.borrow().get(&node.id()) {
            return Doc::Shared(doc.clone());
        }
        let doc = Rc::new(self.build(node));
        self.built.borrow_mut().insert(node.id(), doc.clone());
        Doc::Shared(doc)
    }

    fn build(&self, node: Node) -> Doc {
        if atomic(node, self.code) {
            return text(self.source(node).trim_end());
        }
        match node.kind() {
            "chunk" | "block" => self.statements(node),
            "table_constructor" => self.list(node, true),
            "parameters" => self.list(node, false),
            "arguments" => self.arguments(node),
            "binary_expression" => self.chain(node),
            _ => self.sequence(node),
        }
    }

    /// A block's statements and comments, one a line; a comment on the
    /// line its statement ends stays there.
    fn statements(&self, node: Node) -> Doc {
        let mut docs = Vec::new();
        let mut previous: Option<Node> = None;
        for child in children(node) {
            if let Some(previous) = previous {
                let row = previous.end_position().row;
                if child.kind() == ";" {
                    // Kept against the statement it ends.
                } else if child.kind() == "comment"
                    && child.start_position().row == row
                {
                    docs.push(text(" "));
                } else {
                    docs.push(Doc::Hard);
                    if child.start_position().row > row + 1 {
                        docs.push(Doc::Hard);
                    }
                }
            }
            docs.push(self.node(child));
            previous = Some(child);
        }
        Doc::Concat(docs)
    }

    /// A node's children with the space each pair takes; a block goes on
    /// its own lines, further in, with the comments that close it.
    fn sequence(&self, node: Node) -> Doc {
        let mut docs = Vec::new();
        let mut previous: Option<Node> = None;
        // Whatever follows a block (`end`, `until`, `else`) starts a
        // line of its own.
        let mut after_block = false;
        let kids = children(node);
        let mut index = 0;
        while index < kids.len() {
            let child = kids[index];
            if child.kind() == "block" {
                let mut inner = vec![Doc::Hard, self.statements(child)];
                let mut last = child;
                while let Some(&next) = kids.get(index + 1)
                    && next.kind() == "comment"
                    && kids.get(index + 2).is_some()
                {
                    inner.push(self.separator(last, next, node));
                    inner.push(self.node(next));
                    last = next;
                    index += 1;
                }
                if child.child_count() > 0 || last != child {
                    docs.push(indent(Doc::Concat(inner)));
                }
                after_block = true;
                previous = None;
                index += 1;
                continue;
            }
            if after_block
                || matches!(child.kind(), "elseif_statement" | "else_statement")
                || previous.is_some_and(|p| ends_in_block(p))
            {
                docs.push(Doc::Hard);
            } else if let Some(previous) = previous {
                docs.push(self.separator(previous, child, node));
            }
            after_block = false;
            docs.push(self.node(child));
            previous = Some(child);
            index += 1;
        }
        Doc::Concat(docs)
    }

    /// What goes between two neighbors in `parent`: a space, nothing, or
    /// a line break a comment needs.
    fn separator(&self, before: Node, after: Node, parent: Node) -> Doc {
        let last = last_leaf(before, self.code);
        let first = first_leaf(after, self.code);
        // A line comment's next token is on a later line, so it breaks.
        if first.kind() == "comment" || last.kind() == "comment" {
            return if first.start_position().row > last.end_position().row {
                Doc::Hard
            } else {
                text(" ")
            };
        }
        if self.tight(last, first, after, parent) {
            Doc::Concat(Vec::new())
        } else {
            text(" ")
        }
    }

    /// Whether two tokens are written with nothing between them.
    fn tight(
        &self,
        last: Node,
        first: Node,
        after: Node,
        parent: Node,
    ) -> bool {
        let (left, right) = (self.source(last), self.source(first));
        // Nothing may join two tokens into another: `- -x` is not a
        // comment, `t[ [[s]] ]` not a long bracket.
        if (left.ends_with('-') && right.starts_with('-'))
            || (left.ends_with('[')
                && (right.starts_with('[') || right.starts_with('=')))
        {
            return false;
        }
        match right {
            "," | ";" | ")" | "]" | "." | ":" => return true,
            "?" if owner(first) == "optional_type" => return true,
            "<" | ">" if owner(first) != "binary_expression" => return true,
            "(" if matches!(after.kind(), "arguments" | "parameters") => {
                return true;
            }
            "[" if parent.kind() == "bracket_index_expression" => return true,
            _ => {}
        }
        match left {
            "(" | "[" | "." => return true,
            ":" if owner(last) == "method_index_expression" => return true,
            "<" if owner(last) != "binary_expression" => return true,
            "-" | "#" if owner(last) == "unary_expression" => return true,
            _ => {}
        }
        // A call on a string or a table (`require "x"`, `f{}`) keeps
        // whether it had a space.
        if after.kind() == "arguments" && right != "(" {
            return last.end_byte() == first.start_byte();
        }
        false
    }

    /// A run of one binary operator (`a .. b .. c`, `x and y and z`) on
    /// one line when it fits, and otherwise one operand a line, each
    /// after its operator.
    fn chain(&self, node: Node) -> Doc {
        let kids = children(node);
        let [_, operator, _] = kids[..] else {
            return self.sequence(node);
        };
        let mut parts = Vec::new();
        self.operands(node, self.source(operator), &mut parts);
        let Some(((_, first), rest)) = parts.split_first() else {
            return self.sequence(node);
        };
        let mut docs = Vec::new();
        for (operator, operand) in rest {
            docs.push(Doc::Line);
            if let Some(operator) = operator {
                docs.push(self.node(*operator));
                docs.push(text(" "));
            }
            docs.push(self.node(*operand));
        }
        group(Doc::Concat(vec![
            self.node(*first),
            indent(Doc::Concat(docs)),
        ]))
    }

    /// `node`'s operands as a run of `op`, each after its operator (the
    /// first after none). A node holding a comment is one operand.
    fn operands<'t>(
        &self,
        node: Node<'t>,
        op: &str,
        out: &mut Vec<(Option<Node<'t>>, Node<'t>)>,
    ) {
        let kids = children(node);
        if node.kind() == "binary_expression"
            && let [left, operator, right] = kids[..]
            && self.source(operator) == op
        {
            self.operands(left, op, out);
            let from = out.len();
            self.operands(right, op, out);
            out[from].0 = Some(operator);
        } else {
            out.push((None, node));
        }
    }

    /// A call's arguments: hugged when the last is a table or a
    /// function, so `f(x, function() ... end)` breaks only inside.
    fn arguments(&self, node: Node) -> Doc {
        let kids = children(node);
        if kids.first().map(|k| k.kind()) != Some("(") {
            return self.sequence(node);
        }
        let items: Vec<Node> =
            kids.iter().copied().filter(|k| k.is_named()).collect();
        let hug = items.last().is_some_and(|last| {
            matches!(last.kind(), "table_constructor" | "function_definition")
        }) && items.iter().all(|k| k.kind() != "comment");
        let Some(&last) = items.last().filter(|_| hug) else {
            return self.list(node, false);
        };
        let mut hugged = Vec::new();
        for child in kids {
            if child == last {
                hugged.push(doc::hug(self.node(child)));
            } else {
                hugged.push(self.node(child));
                if child.kind() == "," {
                    hugged.push(text(" "));
                }
            }
        }
        doc::either(Doc::Concat(hugged), self.list(node, false))
    }

    /// Bracketed items, on one line when they fit and one a line when
    /// not; `spaced` ones have a space inside the brackets on one line.
    fn list(&self, node: Node, spaced: bool) -> Doc {
        let kids = children(node);
        let (Some(open), Some(close)) = (kids.first(), kids.last()) else {
            return self.sequence(node);
        };
        let inner = &kids[1..kids.len() - 1];
        if inner.is_empty() {
            return Doc::Concat(vec![self.node(*open), self.node(*close)]);
        }
        let between = || if spaced { Doc::Line } else { Doc::Soft };
        // A table ends in a comma when it breaks and in none on one line,
        // whatever it was written with.
        let table = node.kind() == "table_constructor";
        let last_item = inner
            .iter()
            .rposition(|k| k.is_named() && k.kind() != "comment");
        let after_comment = |at: usize| {
            last_item.is_some_and(|last| {
                inner[last..at].iter().any(|k| k.kind() == "comment")
            })
        };
        let mut docs = vec![between()];
        let mut previous: Option<Node> = None;
        for (at, &child) in inner.iter().enumerate() {
            let trailing = table
                && matches!(child.kind(), "," | ";")
                && last_item.is_some_and(|last| at > last);
            if trailing {
                // After a comment it moves up, to just after the last
                // field (below); the comment breaks the table anyway.
                if !after_comment(at) {
                    docs.push(Doc::Broken(self.source(child).to_owned()));
                    previous = Some(child);
                }
                continue;
            }
            if let Some(previous) = previous {
                let last = last_leaf(previous, self.code);
                if line_comment(last, self.code) {
                    docs.push(Doc::Hard);
                } else if matches!(child.kind(), "," | ";") {
                    // A separator stays against its item.
                } else if child.kind() == "comment" {
                    docs.push(self.separator(previous, child, node));
                } else if matches!(previous.kind(), "," | ";" | "comment") {
                    docs.push(Doc::Line);
                } else {
                    docs.push(self.separator(previous, child, node));
                }
            }
            docs.push(self.node(child));
            previous = Some(child);
            let ended = inner[at + 1..]
                .iter()
                .take_while(|k| k.kind() != "comment")
                .any(|k| matches!(k.kind(), "," | ";"));
            if table && last_item == Some(at) && !ended {
                docs.push(Doc::Broken(",".into()));
            }
        }
        let close_doc = match previous {
            Some(last)
                if line_comment(last_leaf(last, self.code), self.code) =>
            {
                Doc::Hard
            }
            _ => between(),
        };
        group(Doc::Concat(vec![
            self.node(*open),
            indent(Doc::Concat(docs)),
            close_doc,
            self.node(*close),
        ]))
    }
}
