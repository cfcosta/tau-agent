//! What a ChatGPT plan offers: one model per plan family, the newest
//! version the table knows.

use hegel::{TestCase, generators as gs};
use tau_ai::model::{self, Model, PLAN_FAMILIES};

/// A table row with id `id`; everything but the id is irrelevant here.
fn row(id: String) -> Model {
    Model {
        id,
        ..model::find("gpt-4o")
            .expect("gpt-4o is in the table")
            .clone()
    }
}

/// A version like `6`, `6.1` or `5.6.0`, as its numbers.
fn version(tc: &TestCase) -> Vec<u32> {
    tc.draw(
        gs::vecs(gs::integers::<u32>().max_value(12))
            .min_size(1)
            .max_size(3),
    )
}

/// `[6, 1, 0]` → `[6, 1]`: the version with trailing zeros dropped,
/// written out apart from `tau_ai::model::family_version`.
fn normalized(mut version: Vec<u32>) -> Vec<u32> {
    while version.last() == Some(&0) {
        version.pop();
    }
    version
}

/// A drawn id, with its family and version when it is a family's.
type Drawn = (String, Option<(String, Vec<u32>)>);

/// Any table: ids across the plan's families and others, at any
/// versions, plus ids of other shapes.
fn table(tc: &TestCase) -> Vec<Drawn> {
    let families: Vec<&str> = PLAN_FAMILIES
        .iter()
        .copied()
        .chain(["nova", "mini"])
        .collect();
    let size = tc.draw(gs::integers::<usize>().max_value(24));
    (0..size)
        .map(|_| {
            if tc.draw(gs::integers::<u8>().max_value(5)) == 0 {
                let other = tc.draw(gs::sampled_from(vec![
                    "gpt-4o",
                    "o3",
                    "gpt-5.6",
                    "gpt-5-pro-sol",
                    "gpt-x-sol",
                    "gpt-daybreak-blue-latest",
                ]));
                return (other.to_owned(), None);
            }
            let family = tc.draw(gs::sampled_from(families.clone()));
            let version = version(tc);
            let text = version
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(".");
            (
                format!("gpt-{text}-{family}"),
                Some((family.to_owned(), normalized(version))),
            )
        })
        .collect()
}

/// For any table, the result has at most one model per plan family,
/// in the families' order, and each is its family's newest version;
/// a family with any model in the table is present.
#[hegel::test(test_cases = 300)]
fn one_newest_model_per_family(tc: TestCase) {
    let drawn = table(&tc);
    let rows: Vec<Model> =
        drawn.iter().map(|(id, _)| row(id.clone())).collect();
    let picked = model::newest_per_family(&rows);

    let mut expected = Vec::new();
    for family in PLAN_FAMILIES {
        let newest = drawn
            .iter()
            .filter_map(|(_, parsed)| parsed.as_ref())
            .filter(|(f, _)| f == family)
            .map(|(_, v)| v.clone())
            .max();
        if let Some(newest) = newest {
            expected.push((family.to_owned(), newest));
        }
    }
    let got: Vec<(String, Vec<u32>)> = picked
        .iter()
        .map(|m| {
            let (family, version) = drawn
                .iter()
                .find(|(id, _)| *id == m.id)
                .and_then(|(_, parsed)| parsed.clone())
                .unwrap_or_else(|| panic!("{} is not a plan model", m.id));
            (family, version)
        })
        .collect();
    assert_eq!(got, expected);
}

/// Today's table offers these four, named as the table names them.
#[test]
fn todays_plan_models() {
    let picked: Vec<(&str, &str)> = model::plan_models()
        .iter()
        .map(|m| (m.id.as_str(), m.name.as_str()))
        .collect();
    assert_eq!(
        picked,
        [
            ("gpt-6.1-sol", "GPT-6.1 Sol"),
            ("gpt-6-luna", "GPT-6 Luna"),
            ("gpt-6-astra", "GPT-6 Astra"),
            ("gpt-5.6-terra", "GPT-5.6 Terra"),
        ]
    );
}

#[test]
fn versions_compare_by_number() {
    assert_eq!(
        model::family_version("gpt-6.1-sol"),
        Some(("sol", vec![6, 1]))
    );
    assert_eq!(model::family_version("gpt-6.0-sol"), Some(("sol", vec![6])));
    assert_eq!(model::family_version("gpt-5.6"), None);
    assert_eq!(model::family_version("gpt-x-sol"), None);
    assert_eq!(model::family_version("gpt-5-pro-sol"), None);
}
