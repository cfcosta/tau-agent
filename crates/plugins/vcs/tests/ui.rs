//! tau-vcs's UI: the state its cards keep in a window, and that it keeps
//! to the design language.

use std::collections::{BTreeMap, BTreeSet};

use hegel::generators as gs;
use tau_agent::tool::RunId;
use tau_vcs::ui::Ui;

/// Opening and closing files, and picking changes, over any clicks: a
/// file is open after an odd number of clicks on it in its card, and a
/// log's pick is the last change clicked unless clicked again.
#[hegel::test(test_cases = 200)]
fn cards_keep_what_was_clicked(tc: hegel::TestCase) {
    // (file click?, card, which)
    let clicks: Vec<(bool, u8, u8)> = tc.draw(gs::vecs(hegel::tuples!(
        gs::booleans(),
        gs::integers::<u8>().max_value(2),
        gs::integers::<u8>().max_value(3),
    )));
    let run = RunId("r".into());
    let mut ui = Ui::default();
    let mut open: BTreeSet<(String, String)> = BTreeSet::new();
    let mut picked: BTreeMap<String, String> = BTreeMap::new();
    for (file, card, which) in &clicks {
        let (card, which) = (format!("c{card}"), format!("x{which}"));
        if *file {
            ui.toggle_file(&run, &card, &which);
            let key = (card.clone(), which.clone());
            if !open.remove(&key) {
                open.insert(key);
            }
        } else {
            ui.pick(&run, &card, &which);
            if picked.get(&card) == Some(&which) {
                picked.remove(&card);
            } else {
                picked.insert(card.clone(), which.clone());
            }
        }
    }
    for card in 0..3 {
        let card = format!("c{card}");
        assert_eq!(
            ui.picked(&run, &card),
            picked.get(&card).map(String::as_str)
        );
        for which in 0..4 {
            let which = format!("x{which}");
            assert_eq!(
                ui.file_open(&run, &card, &which),
                open.contains(&(card.clone(), which.clone()))
            );
        }
    }
}

/// The UI takes its look from the kit.
#[test]
fn only_the_kit_holds_design_values() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui");
    let found = tau_ui_kit::design::check(&src, &[]);
    assert!(found.is_empty(), "{}", found.join("\n"));
}
