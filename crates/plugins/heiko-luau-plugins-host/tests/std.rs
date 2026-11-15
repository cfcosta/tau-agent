//! The standard part of `heiko` (`plugins/heiko-std`), from the host's
//! side: what its own Luau tests cannot say.
//!
//! | Property | Kind |
//! |---|---|
//! | `heiko.text.is_space` agrees with Rust on every code point | exhaustive |
//! | every part of the standard is in the `heiko` a plugin requires | example |
//! | a package may not name a file of its own as a part of `heiko` | example |


use std::collections::BTreeMap;

use heiko_luau_plugins_host::runtime::{Files, load};
use heiko_testing::block_on_io;
use tokio_util::sync::CancellationToken;

const PLUGIN: &str = r#"
local heiko = require("heiko")
return heiko.plugin({})
"#;

/// Runs `tests` as a package's test file, and says why it failed.
fn passes(tests: &str) {
    let files = Files {
        source: PLUGIN.into(),
        tests: BTreeMap::from([("std".to_owned(), tests.to_owned())]),
        ..Files::named("std")
    };
    let loaded = block_on_io(load("std", files)).unwrap();
    let results = block_on_io(loaded.test(CancellationToken::new()));
    assert_eq!(results.len(), 1, "{results:?}");
    assert!(results[0].passed, "{:?}", results[0].error);
}

/// White space is what Rust's `char::is_whitespace` says, for every
/// code point there is: what a plugin trims is what the host trims.
#[test]
fn white_space_is_rusts_on_every_code_point() {
    let spaces: Vec<String> = (0..=0x10FFFF_u32)
        .filter_map(char::from_u32)
        .filter(|c| c.is_whitespace())
        .map(|c| (c as u32).to_string())
        .collect();
    assert_eq!(spaces.len(), 25, "Unicode's White_Space");
    passes(&format!(
        r#"
local heiko = require("heiko")
local t = heiko.test
t.case("is_space agrees with Rust", function()
  local want = {{}}
  for _, code in ipairs({{ {} }}) do
    want[code] = true
  end
  for code = 0, 0x10FFFF do
    if heiko.text.is_space(code) ~= (want[code] == true) then
      error(string.format("U+%04X", code))
    end
  end
end)
"#,
        spaces.join(", ")
    ));
}

/// A plugin that requires `heiko` has every part of the standard, each
/// a table of functions or a function.
#[test]
fn every_part_is_in_the_heiko_a_plugin_requires() {
    passes(
        r#"
local heiko = require("heiko")
local t = heiko.test
t.case("the standard is there", function()
  for _, name in ipairs({ "text", "count", "values", "store", "jev" }) do
    t.equal(type(heiko[name]), "table", name)
  end
  t.equal(type(heiko.infer), "function")
  t.equal(type(heiko.now), "function")
  t.equal(heiko.text.trim("  a "), "a")
end)
"#,
    );
}

/// `heiko` and its parts are heiko's own names: a package's file may not
/// take one, so nothing a plugin ships stands in for the standard.
#[test]
fn a_package_may_not_name_a_file_as_a_part_of_heiko() {
    for name in ["heiko_text", "heiko_store", "heiko_anything", "heiko"] {
        let files = Files {
            source: PLUGIN.into(),
            libs: BTreeMap::from([(name.to_owned(), "return {}".to_owned())]),
            ..Files::named("taker")
        };
        let error = files.modules().unwrap_err();
        assert_eq!(error, format!("src/{name}.luau: `{name}` is taken"));
    }
}
