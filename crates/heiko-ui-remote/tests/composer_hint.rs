//! A plugin's suggestion for the next run (`points::COMPOSER_HINT`, ADR
//! 0063): the empty composer of a new run in the repository says it,
//! and Tab puts its prompt in; a repository with none keeps the
//! composer's own words.

use std::sync::Once;

use gpui::{TestAppContext, VisualTestContext, size};
use heiko_ui_plugin::{
    Manifest,
    UiPlugin,
    ViewCx,
    points::{self, ComposerHint},
};
use heiko_ui_remote::{
    Workspace,
    catalog::{Catalog, Repo},
    plugins,
};

const REPO: &str = "heiko-agent";
const OTHER: &str = "docbert";
const PLACEHOLDER: &str = "Retry the flaky sync test";
const PROMPT: &str = "Find why sync_test fails one run in ten, and fix it.";

/// A plugin that suggests one run in [`REPO`], and none elsewhere.
struct Suggests;

impl UiPlugin for Suggests {
    type State = ();
    type Data = ();
    type RepoData = ();
    type Settings = ();

    fn name(&self) -> &'static str {
        "suggests"
    }

    fn declaration(&self) -> heiko_ui_plugin::Declaration {
        heiko_ui_plugin::Declaration::new()
    }

    fn manifest(&self) -> Manifest<Self> {
        Manifest::new().contribute(points::COMPOSER_HINT, hint)
    }
}

fn hint(
    at: &points::AtRepo,
    _: &mut ViewCx<'_, Suggests>,
) -> Option<ComposerHint> {
    (at.repo == REPO).then(|| ComposerHint {
        placeholder: PLACEHOLDER.into(),
        prompt: PROMPT.into(),
    })
}

/// The app's plugins and the one above: a process has one registry.
fn install() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        plugins::install(heiko_plugins::plugins().with(Suggests))
    });
}

/// A window on the new run's screen of `repo`, its composer focused.
fn new_run(
    repo: &str,
    cx: &mut TestAppContext,
) -> (gpui::Entity<Workspace>, VisualTestContext) {
    install();
    cx.update(heiko_ui_remote::init);
    let repo = repo.to_owned();
    let window = cx.add_window(|window, cx| {
        let catalog = Catalog {
            repos: vec![Repo::new(repo.clone(), format!("~/{repo}"))],
            open_repos: vec![repo.clone()],
            ..Default::default()
        };
        let mut workspace =
            Workspace::new("heiko", Vec::new(), catalog, window, cx);
        workspace.start_new_run(window, cx);
        workspace
    });
    let workspace = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(window.into(), cx);
    cx.simulate_resize(size(gpui::px(1200.), gpui::px(800.)));
    cx.run_until_parked();
    (workspace, cx)
}

fn placeholder(ws: &gpui::Entity<Workspace>, cx: &VisualTestContext) -> String {
    ws.read_with(cx, |ws, cx| ws.composer_placeholder(cx).to_owned())
}

fn text(ws: &gpui::Entity<Workspace>, cx: &VisualTestContext) -> String {
    ws.read_with(cx, |ws, cx| ws.composer_text(cx).to_owned())
}

#[gpui::test]
fn the_empty_composer_says_the_suggestion_and_tab_writes_it(
    cx: &mut TestAppContext,
) {
    let (workspace, mut cx) = new_run(REPO, cx);
    assert_eq!(placeholder(&workspace, &cx), PLACEHOLDER);
    cx.simulate_keystrokes("tab");
    cx.run_until_parked();
    assert_eq!(text(&workspace, &cx), PROMPT);
    // With text in it, Tab leaves the suggestion out: the composer takes
    // the key as it always does.
    cx.simulate_keystrokes("space x tab");
    cx.run_until_parked();
    let written = text(&workspace, &cx);
    assert!(written.starts_with(&format!("{PROMPT} x")), "{written:?}");
    assert_eq!(written.matches(PROMPT).count(), 1, "{written:?}");
}

#[gpui::test]
fn a_repository_with_no_suggestion_keeps_the_composer_s_words(
    cx: &mut TestAppContext,
) {
    let (workspace, mut cx) = new_run(OTHER, cx);
    assert_eq!(
        placeholder(&workspace, &cx),
        "Describe a change, paste an issue, or @ a file"
    );
    cx.simulate_keystrokes("tab");
    cx.run_until_parked();
    assert!(!text(&workspace, &cx).contains(PROMPT));
}
