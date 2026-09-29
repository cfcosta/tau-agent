//! Constitutions in the store, against a map of what was last saved for
//! each repository: every read returns exactly that, rules in order, and
//! a save replaces the one before it whole.

use std::collections::BTreeMap;

use hegel::{TestCase, generators as gs};
use tau_store::{Store, StoredConstitution, StoredRule};

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(future)
}

#[hegel::composite]
fn constitution(tc: TestCase) -> StoredConstitution {
    let count = tc.draw(gs::integers::<usize>().max_value(5));
    // Distinct ids, in a drawn order.
    let mut ids: Vec<usize> = (1..=8).collect();
    let rules = (0..count)
        .map(|_| {
            let at = tc.draw(gs::integers::<usize>().max_value(ids.len() - 1));
            StoredRule {
                id: format!("R{}", ids.remove(at)),
                text: tc.draw(gs::text().max_size(40)),
                targets: tc.draw(
                    gs::vecs(gs::sampled_from(vec![
                        "edit.newText".to_owned(),
                        "bash.command".to_owned(),
                        "final answer".to_owned(),
                    ]))
                    .max_size(3),
                ),
                review: tc
                    .draw(gs::floats::<f64>().min_value(0.0).max_value(1.0)),
                block: tc
                    .draw(gs::floats::<f64>().min_value(0.0).max_value(1.0)),
            }
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
    block_on(async {
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
