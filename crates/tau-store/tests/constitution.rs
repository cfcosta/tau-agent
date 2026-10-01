//! Constitutions in the store, against a map of what was last saved for
//! each repository: every read returns exactly that, rules in order, and
//! a save replaces the one before it whole.

use std::collections::BTreeMap;

use hegel::{
    TestCase,
    generators as gs,
    generators::{Generator as _, PrintableGenerator},
};
use tau_store::{Store, StoredConstitution, StoredRule};
use tau_testing::block_on_io;

fn constitution() -> impl PrintableGenerator<StoredConstitution> {
    // StoredConstitution is tau's own type, so its drawn values print through Debug.
    constitution_unprinted().print_as_debug()
}

#[hegel::composite]
fn constitution_unprinted(tc: &TestCase) -> StoredConstitution {
    // Distinct ids, in a drawn order.
    let ids: Vec<usize> = tc.draw(
        gs::samples((1..=8).collect::<Vec<usize>>())
            .without_replacement()
            .max_size(5),
    );
    let rules = ids
        .into_iter()
        .map(|id| StoredRule {
            id: format!("R{id}"),
            text: tc.draw(gs::text().max_size(40)),
            targets: tc.draw(
                gs::vecs(gs::sampled_from(vec![
                    "edit.newText".to_owned(),
                    "bash.command".to_owned(),
                    "final answer".to_owned(),
                ]))
                .max_size(3),
            ),
            review: tc.draw(gs::floats::<f64>().min_value(0.0).max_value(1.0)),
            block: tc.draw(gs::floats::<f64>().min_value(0.0).max_value(1.0)),
        })
        .collect();
    StoredConstitution {
        on_error: tc.draw(gs::sampled_from(vec![
            "allow".to_owned(),
            "block".to_owned(),
        ])),
        max_holds: tc.draw(gs::integers::<u32>().max_value(10)),
        rules,
    }
}

#[hegel::test(test_cases = 100)]
fn a_constitution_reads_back_as_last_saved(tc: TestCase) {
    block_on_io(async {
        let store = Store::memory().await.unwrap();
        let mut saved: BTreeMap<&str, StoredConstitution> = BTreeMap::new();
        for _ in 0..tc.draw(gs::integers::<usize>().max_value(8)) {
            let repo = tc.draw(gs::sampled_from(vec!["/a", "/b", "/c"]));
            if tc.draw(gs::booleans()) {
                let constitution = tc.draw(constitution());
                store.save_constitution(repo, &constitution).await.unwrap();
                saved.insert(repo, constitution);
            }
            for repo in ["/a", "/b", "/c"] {
                assert_eq!(
                    store.constitution(repo).await.unwrap().as_ref(),
                    saved.get(repo),
                    "{repo}"
                );
            }
        }
    });
}
