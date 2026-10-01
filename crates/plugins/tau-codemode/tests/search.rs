//! `search_tools`'s ranking.
//!
//! | Property | Oracle |
//! | --- | --- |
//! | a tool's exact name ranks it first | construction |
//! | a word only one tool has finds only that tool | construction |

use hegel::{TestCase, generators as gs};
use serde_json::json;
use tau_codemode::{ToolEntry, search::Index};

fn tool(name: &str, description: &str) -> ToolEntry {
    ToolEntry {
        name: name.into(),
        description: description.into(),
        input_schema: json!({ "type": "object" }),
        output_schema: None,
        namespace: None,
        sequential: false,
    }
}

#[hegel::composite]
fn named(tc: &TestCase) -> Vec<(String, String)> {
    let names: Vec<String> = tc.draw(
        gs::vecs(gs::from_regex("[a-m]{1,6}(_[a-m]{1,6}){0,2}"))
            .unique(true)
            .min_size(1)
            .max_size(10),
    );
    names
        .into_iter()
        .map(|name| {
            let words: Vec<String> =
                tc.draw(gs::vecs(gs::from_regex("[a-m]{1,7}")).max_size(12));
            (name, words.join(" "))
        })
        .collect()
}

fn tools(named: Vec<(String, String)>) -> Vec<ToolEntry> {
    named.iter().map(|(n, d)| tool(n, d)).collect()
}

#[hegel::test]
fn an_exact_name_ranks_first(tc: TestCase) {
    let tools = tools(tc.draw(named()));
    let pick = tc.draw(gs::integers::<usize>().max_value(tools.len() - 1));
    let index = Index::new(&tools, &[]);
    let found = index.search(&tools[pick].name, 8, None);
    assert_eq!(found.first(), Some(&pick));
}

#[hegel::test]
fn a_word_only_one_tool_has_finds_only_it(tc: TestCase) {
    let mut tools = tools(tc.draw(named()));
    let pick = tc.draw(gs::integers::<usize>().max_value(tools.len() - 1));
    // Letters past `m` appear in no generated name or description.
    tools[pick].description.push_str(" zebras");
    let index = Index::new(&tools, &[]);
    assert_eq!(index.search("zebra", 8, None), [pick]);
}

#[test]
fn tokens_split_camel_case_and_drop_plurals() {
    use tau_codemode::search::tokenize;
    assert_eq!(tokenize("listIssues"), ["list", "issue"]);
    assert_eq!(
        tokenize("HTTPServer for the queries"),
        ["http", "server", "query"]
    );
    assert_eq!(
        tokenize("mcp__linear__get_boxes"),
        ["mcp", "linear", "get", "box"]
    );
}
