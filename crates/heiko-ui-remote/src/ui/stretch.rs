//! Stretches of calls (ADR 0060): the calls between two things that are
//! not calls, drawn as numbered steps on a spine under one head line
//! that folds them.

use heiko_ui_kit::theme::ToolKind;

use crate::view::{Item, ToolCard, ToolState};

/// Whether `item` keeps a stretch going: a call, or what sits between
/// two of a model's calls (a turn's end, its reasoning).
fn within(item: &Item) -> bool {
    matches!(
        item,
        Item::Tool(_) | Item::TurnEnd { .. } | Item::Thinking { .. }
    )
}

/// A stretch: the items from its first call to its last, as indexes
/// into a run's items.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stretch {
    /// Its first call's index.
    pub first: usize,
    /// Its last call's index.
    pub last: usize,
}

impl Stretch {
    /// The stretch the item at `index` is in, if it is in one: a call,
    /// or what sits between two calls of one stretch.
    pub fn around(items: &[Item], index: usize) -> Option<Self> {
        if !within(items.get(index)?) {
            return None;
        }
        let is_call = |at: usize| matches!(items.get(at), Some(Item::Tool(_)));
        let mut first = None;
        let mut at = index;
        loop {
            if is_call(at) {
                first = Some(at);
            }
            match at.checked_sub(1) {
                Some(before) if within(&items[before]) => at = before,
                _ => break,
            }
        }
        let mut last = None;
        let mut at = index;
        while at < items.len() && within(&items[at]) {
            if is_call(at) {
                last = Some(at);
            }
            at += 1;
        }
        let (first, last) = (first?, last?);
        (first <= index && index <= last).then_some(Self { first, last })
    }

    /// Its calls, with their indexes.
    pub fn calls<'a>(
        self,
        items: &'a [Item],
    ) -> impl Iterator<Item = (usize, &'a ToolCard)> + 'a {
        items[self.first..=self.last].iter().enumerate().filter_map(
            move |(n, item)| match item {
                Item::Tool(card) => Some((self.first + n, &**card)),
                _ => None,
            },
        )
    }

    /// Whether it has a head line: it holds more than one call.
    pub fn headed(&self, items: &[Item]) -> bool {
        self.calls(items).nth(1).is_some()
    }

    /// Whether it starts folded: something not a call came after it,
    /// and none of its calls is still going.
    pub fn folds_by_default(&self, items: &[Item]) -> bool {
        let followed = items[self.last + 1..].iter().any(|item| !within(item));
        followed
            && self
                .calls(items)
                .all(|(_, card)| !matches!(card.state, ToolState::Running))
    }
}

/// The number of the call at `index`: how many calls there are since
/// the person's last message, it included.
pub fn step(items: &[Item], index: usize) -> usize {
    items[..=index.min(items.len().saturating_sub(1))]
        .iter()
        .rev()
        .take_while(|item| !matches!(item, Item::User(_)))
        .filter(|item| matches!(item, Item::Tool(_)))
        .count()
}

/// What the tool of `card` does: as its result says (a Luau plugin's
/// tool), or as heiko knows its own.
pub fn kind(card: &ToolCard) -> Option<ToolKind> {
    card.data
        .result
        .as_ref()
        .and_then(|result| result.details.as_ref())
        .and_then(|details| details.get("kind"))
        .and_then(|kind| kind.as_str())
        .and_then(ToolKind::named)
        .or_else(|| ToolKind::of(&card.tool))
}

/// Whether the call of `card` failed or was refused.
pub fn failed(card: &ToolCard) -> bool {
    matches!(card.state, ToolState::Failed(_) | ToolState::Blocked { .. })
        || card.data.result.as_ref().is_some_and(|result| result.error)
}

/// What a stretch's calls did, by tool, in the order each first came:
/// `read · grep · edit ×2`, the first `shown` tools and how many more.
pub fn verbs<'a>(
    cards: impl Iterator<Item = &'a ToolCard>,
    shown: usize,
) -> String {
    let mut counted: Vec<(&str, usize)> = Vec::new();
    for card in cards {
        match counted.iter_mut().find(|(tool, _)| *tool == card.tool) {
            Some((_, n)) => *n += 1,
            None => counted.push((&card.tool, 1)),
        }
    }
    let mut said: Vec<String> = counted
        .iter()
        .take(shown)
        .map(|(tool, n)| match n {
            1 => (*tool).to_owned(),
            n => format!("{tool} ×{n}"),
        })
        .collect();
    if counted.len() > shown {
        said.push(format!("{} more", counted.len() - shown));
    }
    said.join(" · ")
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use heiko_ui_plugin::{CallData, CallResult};
    use serde_json::json;

    use super::*;

    fn call(tool: &str, state: ToolState) -> Item {
        Item::Tool(Box::new(ToolCard {
            call_id: format!("c-{tool}"),
            tool: tool.into(),
            summary: String::new(),
            state,
            data: Arc::new(CallData::default()),
            from_plugin: None,
            dropped: None,
            cut: None,
            size: 0,
            anchors: Vec::new(),
        }))
    }

    fn done(tool: &str) -> Item {
        call(tool, ToolState::Done { summary: None })
    }

    /// The person's message, then a stretch of calls over two turns with
    /// reasoning between, then the model's words and one more call.
    fn turn() -> Vec<Item> {
        vec![
            Item::User("go".into()),
            done("read"),
            Item::TurnEnd { turn: 1 },
            Item::Thinking {
                text: "hm".into(),
                secs: Some(1),
            },
            done("grep"),
            done("edit"),
            Item::Text("Now the tests.".into()),
            call("exec_command", ToolState::Running),
        ]
    }

    #[test]
    fn a_stretch_runs_from_its_first_call_to_its_last() {
        let items = turn();
        let first = Stretch { first: 1, last: 5 };
        for index in 1..=5 {
            assert_eq!(Stretch::around(&items, index), Some(first), "{index}");
        }
        assert_eq!(Stretch::around(&items, 0), None);
        assert_eq!(Stretch::around(&items, 6), None);
        let lone = Stretch::around(&items, 7).unwrap();
        assert_eq!(lone, Stretch { first: 7, last: 7 });
        assert!(first.headed(&items));
        assert!(!lone.headed(&items));
    }

    /// A turn's end with no call after it is not in a stretch.
    #[test]
    fn what_trails_the_last_call_is_not_in_the_stretch() {
        let items = vec![done("read"), Item::TurnEnd { turn: 1 }];
        assert_eq!(Stretch::around(&items, 1), None);
        assert_eq!(
            Stretch::around(&items, 0),
            Some(Stretch { first: 0, last: 0 })
        );
    }

    #[test]
    fn steps_count_from_the_persons_last_message() {
        let mut items = turn();
        let steps: Vec<usize> = [1, 4, 5, 7]
            .into_iter()
            .map(|index| step(&items, index))
            .collect();
        assert_eq!(steps, [1, 2, 3, 4]);
        items.push(Item::User("again".into()));
        items.push(done("ls"));
        assert_eq!(step(&items, items.len() - 1), 1);
    }

    /// A finished stretch the model spoke after folds; the stretch at
    /// the run's end, or one with a call still going, stays open.
    #[test]
    fn a_stretch_folds_once_something_follows_it_and_its_calls_end() {
        let items = turn();
        assert!(Stretch { first: 1, last: 5 }.folds_by_default(&items));
        assert!(!Stretch { first: 7, last: 7 }.folds_by_default(&items));
        let mut going = turn();
        going[4] = call("grep", ToolState::Running);
        assert!(!Stretch { first: 1, last: 5 }.folds_by_default(&going));
    }

    #[test]
    fn verbs_count_each_tool_in_the_order_it_came() {
        let items = [done("read"), done("grep"), done("edit"), done("edit")];
        let cards = || {
            items.iter().filter_map(|item| match item {
                Item::Tool(card) => Some(&**card),
                _ => None,
            })
        };
        assert_eq!(verbs(cards(), 4), "read · grep · edit ×2");
        assert_eq!(verbs(cards(), 2), "read · grep · 1 more");
    }

    /// A Luau tool's result says its kind; heiko's own tools have theirs
    /// by name.
    #[test]
    fn a_calls_kind_is_its_results_or_its_names() {
        let Item::Tool(mut card) = done("todo_create") else {
            unreachable!()
        };
        assert_eq!(kind(&card), None);
        card.data = Arc::new(CallData {
            result: Some(CallResult {
                text: String::new(),
                details: Some(json!({ "kind": "change" })),
                error: false,
            }),
            ..CallData::default()
        });
        assert_eq!(kind(&card), Some(ToolKind::Change));
        card.tool = "mcp__linear__create_issue".into();
        card.data = Arc::new(CallData::default());
        assert_eq!(kind(&card), Some(ToolKind::Reach));
    }
}
