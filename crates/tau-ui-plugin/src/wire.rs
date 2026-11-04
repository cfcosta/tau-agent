//! What names a plugin's view between a phone and the computer (ADR
//! 0046): the phone asks for a view by its [`ViewKey`], and the computer
//! builds it from its own state and sends it back. A point's context
//! crosses as an [`AtKey`], the names the computer rebuilds it from.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use tau_agent::tool::RunId;

/// Where a point's context is, by name: what the computer rebuilds it
/// from, out of the runs and repositories both sides have.
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(tag = "at", rename_all = "snake_case")]
pub enum AtKey {
    App,
    Repo {
        repo: String,
    },
    Run {
        run: RunId,
    },
    /// A message, by its place among the run's items.
    Message {
        run: RunId,
        index: usize,
    },
    /// A plugin's anchor in the transcript, by its key and place.
    Anchor {
        run: RunId,
        key: String,
        index: usize,
    },
    /// A rewrite a plugin named, by its key and place.
    Rewrite {
        run: RunId,
        key: String,
        index: usize,
    },
    /// A tool call's card.
    Card {
        run: RunId,
        call: String,
    },
    /// Changes on a run's stack: a landing's. They are few, and the
    /// computer may no longer have them, so they cross whole.
    Changes {
        run: RunId,
        changes: Vec<crate::vcs::ChangeInfo>,
    },
}

/// What a view is of.
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(tag = "view", rename_all = "snake_case")]
pub enum View {
    /// What every plugin contributes at the point named `point`, at `at`.
    Point { point: String, at: AtKey },
    /// `plugin`'s page `page`, with its parameters.
    Page {
        plugin: String,
        page: String,
        params: BTreeMap<String, String>,
    },
    /// `plugin`'s settings pane, for a repository's copy or everywhere,
    /// and the entry it is of.
    Settings {
        plugin: String,
        scope: Option<String>,
        entry: Option<String>,
    },
    /// The popover of `plugin`'s command while it is written, with what
    /// follows its name, on the run open.
    Popover {
        plugin: String,
        command: String,
        args: String,
        run: Option<RunId>,
    },
    /// The commands every plugin lists where the composer is.
    Commands { repo: Option<String> },
    /// The title of `plugin`'s page `page`, with its parameters.
    Title {
        plugin: String,
        page: String,
        params: BTreeMap<String, String>,
    },
}

/// How the window that draws a view is laid out: what a view may read
/// of it.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Default,
    Serialize,
    Deserialize,
)]
pub struct Layout {
    /// Whether the window has the phone's layout.
    pub compact: bool,
    /// Its width, in steps of [`Layout::STEP`] pixels, so a view built
    /// for a width is not built again for each pixel.
    pub width: u32,
}

impl Layout {
    /// The pixels of a width's step.
    pub const STEP: f32 = 40.;

    /// The layout of a window `width` pixels wide.
    pub fn of(compact: bool, width: f32) -> Self {
        Self {
            compact,
            width: (width.max(0.) / Self::STEP).round() as u32,
        }
    }

    /// The width a view reads: its step's pixels.
    pub fn width(&self) -> f32 {
        self.width as f32 * Self::STEP
    }
}

/// A view the phone asks for: what it is of, and the layout it is drawn
/// in.
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
pub struct ViewKey {
    pub view: View,
    pub layout: Layout,
}

/// One plugin's output in a view of a point: its plugin, and the output
/// as JSON, of the point's own type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Contributed {
    pub plugin: String,
    pub out: serde_json::Value,
}

/// A point's output as it crosses: what the computer sends of it, and
/// what the phone reads back. Setup's steps stay on the computer.
pub trait Wired: Sized + 'static {
    fn to_wire(&self) -> Option<serde_json::Value>;
    fn from_wire(value: serde_json::Value) -> Option<Self>;
}

macro_rules! wired {
    ($($out:ty),* $(,)?) => {$(
        impl Wired for $out {
            fn to_wire(&self) -> Option<serde_json::Value> {
                serde_json::to_value(self).ok()
            }

            fn from_wire(value: serde_json::Value) -> Option<Self> {
                serde_json::from_value(value).ok()
            }
        }
    )*};
}

wired!(
    crate::tree::Node,
    crate::NavEntry,
    crate::PluginStatus,
    crate::PlanField,
    crate::RowNote,
    crate::points::CardView,
    crate::points::PaneView,
    f32,
    String,
);

impl Wired for crate::points::SetupView {
    fn to_wire(&self) -> Option<serde_json::Value> {
        None
    }

    fn from_wire(_: serde_json::Value) -> Option<Self> {
        None
    }
}

/// A view, built: what the phone draws for its [`ViewKey`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "built", rename_all = "snake_case")]
pub enum Built {
    /// Each plugin's output at a point, in order.
    Outs { outs: Vec<Contributed> },
    /// A page's tree, and whether the interface frames it.
    Page {
        tree: crate::tree::Node,
        framed: bool,
    },
    /// A settings pane's or a popover's tree.
    Tree { tree: crate::tree::Node },
    /// Each plugin's commands where the composer is, by plugin.
    Commands {
        commands: Vec<(String, crate::registry::CommandInfo)>,
    },
    /// A page's title.
    Title { title: String },
    /// Nothing is there: no such plugin, page, run or call.
    Nothing,
}

#[cfg(test)]
mod tests {
    use hegel::{TestCase, generators as gs};

    use super::*;

    fn word(tc: &TestCase) -> String {
        tc.draw(gs::from_regex("[a-z0-9/._-]{0,8}").fullmatch(true))
    }

    fn run(tc: &TestCase) -> RunId {
        RunId(word(tc).into())
    }

    fn at(tc: &TestCase) -> AtKey {
        let index = tc.draw(gs::integers::<usize>().max_value(1_000));
        match tc.draw(gs::integers::<u8>().max_value(7)) {
            0 => AtKey::App,
            1 => AtKey::Repo { repo: word(tc) },
            2 => AtKey::Run { run: run(tc) },
            3 => AtKey::Message {
                run: run(tc),
                index,
            },
            4 => AtKey::Anchor {
                run: run(tc),
                key: word(tc),
                index,
            },
            5 => AtKey::Rewrite {
                run: run(tc),
                key: word(tc),
                index,
            },
            6 => AtKey::Card {
                run: run(tc),
                call: word(tc),
            },
            _ => AtKey::Changes {
                run: run(tc),
                changes: (0..tc.draw(gs::integers::<usize>().max_value(3)))
                    .map(|_| change(tc))
                    .collect(),
            },
        }
    }

    fn change(tc: &TestCase) -> crate::vcs::ChangeInfo {
        crate::vcs::ChangeInfo {
            change_id: word(tc),
            commit_id: word(tc),
            description: word(tc),
            empty: tc.draw(gs::booleans()),
            conflict: tc.draw(gs::booleans()),
            immutable: tc.draw(gs::booleans()),
            working_copy: tc.draw(gs::booleans()),
            divergent: tc.draw(gs::booleans()),
            bookmarks: (0..tc.draw(gs::integers::<usize>().max_value(2)))
                .map(|_| word(tc))
                .collect(),
        }
    }

    fn maybe(tc: &TestCase) -> Option<String> {
        tc.draw(gs::booleans()).then(|| word(tc))
    }

    fn view(tc: &TestCase) -> View {
        match tc.draw(gs::integers::<u8>().max_value(4)) {
            0 => View::Point {
                point: word(tc),
                at: at(tc),
            },
            1 => View::Page {
                plugin: word(tc),
                page: word(tc),
                params: (0..tc.draw(gs::integers::<usize>().max_value(3)))
                    .map(|_| (word(tc), word(tc)))
                    .collect(),
            },
            2 => View::Settings {
                plugin: word(tc),
                scope: maybe(tc),
                entry: maybe(tc),
            },
            3 => View::Popover {
                plugin: word(tc),
                command: word(tc),
                args: word(tc),
                run: tc.draw(gs::booleans()).then(|| run(tc)),
            },
            _ => View::Commands { repo: maybe(tc) },
        }
    }

    /// Every key reads back from JSON as it was written, so the phone
    /// and the computer name the same view by it.
    #[hegel::test(test_cases = 300)]
    fn a_view_key_reads_back_what_it_wrote(tc: TestCase) {
        let key = ViewKey {
            view: view(&tc),
            layout: Layout {
                compact: tc.draw(gs::booleans()),
                width: tc.draw(gs::integers::<u32>().max_value(200)),
            },
        };
        let json = serde_json::to_value(&key).unwrap();
        assert_eq!(serde_json::from_value::<ViewKey>(json).unwrap(), key);
    }

    fn tone(tc: &TestCase) -> tau_ui_kit::theme::Tone {
        use tau_ui_kit::theme::Tone;
        let tones = [
            Tone::Quiet,
            Tone::Good,
            Tone::Warn,
            Tone::Danger,
            Tone::Info,
        ];
        tones[tc.draw(gs::integers::<usize>().max_value(tones.len() - 1))]
    }

    fn node(tc: &TestCase) -> crate::tree::Node {
        use crate::tree::{Kind, Node};
        match tc.draw(gs::integers::<u8>().max_value(3)) {
            0 => Node::text(word(tc)),
            1 => Node::Mono { text: word(tc) },
            2 => Node::Button {
                label: word(tc),
                action: word(tc),
                args: serde_json::json!({ "n": tc.draw(gs::integers::<i32>()) }),
                kind: Kind::Secondary,
                icon: maybe(tc),
            },
            _ => Node::stack([Node::text(word(tc)), Node::Rule]),
        }
    }

    /// What a view gives and what an action asks read back from JSON as
    /// they were written: the typed outputs, with their icons and tones,
    /// and the requests a phone's window carries out.
    #[hegel::test(test_cases = 200)]
    fn outputs_and_requests_read_back_what_they_wrote(tc: TestCase) {
        use tau_ui_kit::assets::Icon;

        use crate::{
            NavEntry,
            Request,
            RowNote,
            points::{CardView, PaneView},
            view::Link,
        };

        fn back<T>(value: &T) -> T
        where
            T: Serialize + serde::de::DeserializeOwned,
        {
            serde_json::from_value(serde_json::to_value(value).unwrap())
                .unwrap()
        }

        let icons = [Icon::Memory, Icon::SubAgent, Icon::Plug];
        let icon =
            icons[tc.draw(gs::integers::<usize>().max_value(icons.len() - 1))];
        let entry = NavEntry::new(word(&tc), icon, Link::page(word(&tc)))
            .detail(word(&tc))
            .badge(tc.draw(gs::booleans()).then(|| (word(&tc), tone(&tc))));
        assert_eq!(back(&entry), entry);
        let note = RowNote {
            line: maybe(&tc),
            phone_line: maybe(&tc),
            count: maybe(&tc),
            icon,
            tone: tone(&tc),
        };
        assert_eq!(back(&note), note);
        let card = CardView {
            head: tc.draw(gs::booleans()).then(|| node(&tc)),
            label: maybe(&tc),
            failed: maybe(&tc),
            edge: tc.draw(gs::booleans()).then(|| tone(&tc)),
            shape: None,
            body: tc.draw(gs::booleans()).then(|| node(&tc)),
            folds: tc.draw(gs::booleans()),
            inset: tc.draw(gs::booleans()),
        };
        assert_eq!(back(&card), card);
        let pane = PaneView {
            title: word(&tc),
            fresh: tc.draw(gs::integers::<u64>()),
            body: node(&tc),
        };
        assert_eq!(back(&pane), pane);
        let request = match tc.draw(gs::integers::<u8>().max_value(5)) {
            0 => Request::Composer(word(&tc)),
            1 => Request::Navigate(
                Link::page(word(&tc)).param("repo", word(&tc)),
            ),
            2 => Request::Act(serde_json::json!([word(&tc)])),
            3 => Request::OpenRun(run(&tc)),
            4 => Request::Ui {
                key: word(&tc),
                value: tc
                    .draw(gs::booleans())
                    .then(|| serde_json::json!(word(&tc))),
            },
            _ => Request::Reveal(word(&tc).into()),
        };
        assert_eq!(back(&request), request);
    }

    /// A layout keeps the width's step, and widths in one step make one
    /// layout.
    #[hegel::test(test_cases = 200)]
    fn widths_in_a_step_make_one_layout(tc: TestCase) {
        let width = tc.draw(gs::floats::<f32>().min_value(0.).max_value(4000.));
        let layout = Layout::of(true, width);
        assert!((layout.width() - width).abs() <= Layout::STEP / 2. + 0.01);
        assert_eq!(Layout::of(true, layout.width()), layout);
    }
}
