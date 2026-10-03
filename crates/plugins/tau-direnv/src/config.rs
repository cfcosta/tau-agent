//! tau's own direnv configuration: the person's, with the repositories
//! they allowed added to its whitelist, so direnv loads them without
//! `direnv allow` and without tau touching the person's own files.
//!
//! direnv reads its configuration from `DIRENV_CONFIG`; tau points it at
//! a directory of its own holding:
//!
//! - `direnv.toml`: the person's (`direnv.toml`, or the older
//!   `config.toml`) with every key kept, and the allowed repositories'
//!   directories added to `[whitelist] prefix`;
//! - `lib` and `direnvrc`: links to the person's, when they have them,
//!   so their extensions (nix-direnv) still load.
//!
//! direnv's allow and deny records stay where they are: a `direnv deny`
//! of the person's still holds.

use std::path::{Path, PathBuf};

use toml::{Table, Value};

/// The person's direnv configuration directory: `DIRENV_CONFIG`, else
/// `$XDG_CONFIG_HOME/direnv`, else `~/.config/direnv`, as direnv finds
/// it.
pub fn user_dir() -> Option<PathBuf> {
    let var = |name| std::env::var_os(name).filter(|value| !value.is_empty());
    if let Some(dir) = var("DIRENV_CONFIG") {
        return Some(PathBuf::from(dir));
    }
    if let Some(config) = var("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(config).join("direnv"));
    }
    var("HOME").map(|home| PathBuf::from(home).join(".config/direnv"))
}

/// The text of the person's configuration file in `dir`:
/// `direnv.toml`, else `config.toml`; none when there is neither.
pub fn user_toml(dir: &Path) -> Option<String> {
    ["direnv.toml", "config.toml"]
        .iter()
        .find_map(|name| std::fs::read_to_string(dir.join(name)).ok())
}

/// `user`'s configuration with `roots` added to `[whitelist] prefix`,
/// after the prefixes it has: every other key as it was, and each root
/// once. Fails when `user` is not TOML, or its whitelist is not a table
/// of arrays.
pub fn merged(user: Option<&str>, roots: &[PathBuf]) -> Result<String, String> {
    let mut table: Table = match user {
        Some(text) => text
            .parse()
            .map_err(|error| format!("direnv.toml does not parse: {error}"))?,
        None => Table::new(),
    };
    let whitelist = table
        .entry("whitelist")
        .or_insert_with(|| Value::Table(Table::new()))
        .as_table_mut()
        .ok_or("direnv.toml's `whitelist` is not a table")?;
    let prefix = whitelist
        .entry("prefix")
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .ok_or("direnv.toml's `whitelist.prefix` is not an array")?;
    for root in roots {
        let root = Value::String(root.display().to_string());
        if !prefix.contains(&root) {
            prefix.push(root);
        }
    }
    toml::to_string(&table).map_err(|error| error.to_string())
}

/// Writes tau's configuration into `dir` from the person's in `user`
/// (none when they have no directory), allowing `roots`: the merged
/// `direnv.toml`, and links to their `lib` and `direnvrc`.
pub fn write(
    dir: &Path,
    user: Option<&Path>,
    roots: &[PathBuf],
) -> Result<(), String> {
    let text = merged(user.and_then(user_toml).as_deref(), roots)?;
    std::fs::create_dir_all(dir).map_err(|error| error.to_string())?;
    // Written whole, then moved, so a direnv reading it never sees half.
    let staged = dir.join("direnv.toml.new");
    std::fs::write(&staged, text).map_err(|error| error.to_string())?;
    std::fs::rename(&staged, dir.join("direnv.toml"))
        .map_err(|error| error.to_string())?;
    for name in ["lib", "direnvrc"] {
        let link = dir.join(name);
        let _ = std::fs::remove_file(&link);
        if let Some(target) = user
            .map(|user| user.join(name))
            .filter(|target| target.exists())
        {
            std::os::unix::fs::symlink(&target, &link)
                .map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use hegel::{
        TestCase,
        generators::{self as gs, Generator as _},
    };

    use super::*;

    /// A key or a word: letters, so it reads back the same.
    fn word() -> impl gs::PrintableGenerator<String> {
        gs::from_regex("[a-z][a-z_]{0,7}").fullmatch(true)
    }

    /// A person's configuration: some sections of scalar keys, and maybe
    /// a whitelist with prefixes and exact paths of its own.
    #[hegel::composite]
    fn user_config(tc: &TestCase) -> Table {
        let mut table = Table::new();
        let sections: Vec<String> = tc.draw(gs::vecs(word()).max_size(3));
        for section in sections.into_iter().filter(|name| name != "whitelist") {
            let mut keys = Table::new();
            let pairs: Vec<(String, bool)> = tc.draw(
                gs::vecs(gs::tuples2(word(), gs::booleans())).max_size(4),
            );
            for (key, value) in pairs {
                keys.insert(key, Value::Boolean(value));
            }
            table.insert(section, Value::Table(keys));
        }
        if tc.draw(gs::booleans()) {
            let mut whitelist = Table::new();
            let paths = |tc: &TestCase| -> Vec<Value> {
                let words: Vec<String> = tc.draw(gs::vecs(word()).max_size(3));
                words
                    .into_iter()
                    .map(|w| Value::String(format!("/home/{w}")))
                    .collect()
            };
            if tc.draw(gs::booleans()) {
                whitelist.insert("prefix".into(), Value::Array(paths(tc)));
            }
            if tc.draw(gs::booleans()) {
                whitelist.insert("exact".into(), Value::Array(paths(tc)));
            }
            table.insert("whitelist".into(), Value::Table(whitelist));
        }
        table
    }

    fn prefixes(table: &Table) -> Vec<String> {
        table
            .get("whitelist")
            .and_then(|w| w.get("prefix"))
            .and_then(Value::as_array)
            .map(|array| {
                array
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Every key of the person's survives, their prefixes come first,
    /// then each allowed root once and nothing else; merging again with
    /// the same roots changes nothing.
    #[hegel::test(test_cases = 200)]
    fn the_merge_keeps_the_persons_keys_and_adds_the_roots(tc: TestCase) {
        let user = tc.draw(user_config().print_as_debug());
        let words: Vec<String> = tc.draw(gs::vecs(word()).max_size(4));
        let roots: Vec<PathBuf> = words
            .iter()
            .map(|w| PathBuf::from(format!("/tau/{w}")))
            .collect();
        let text = toml::to_string(&user).unwrap();
        let merged_text = merged(Some(&text), &roots).unwrap();
        let out: Table = merged_text.parse().unwrap();

        // Every key but the whitelist's prefix is as it was.
        for (key, value) in &user {
            if key == "whitelist" {
                let before = value.as_table().unwrap();
                let after = out["whitelist"].as_table().unwrap();
                for (inner, value) in
                    before.iter().filter(|(k, _)| *k != "prefix")
                {
                    assert_eq!(
                        after.get(inner),
                        Some(value),
                        "whitelist.{inner}"
                    );
                }
            } else {
                assert_eq!(out.get(key), Some(value), "{key}");
            }
        }
        assert_eq!(
            out.len(),
            user.len() + usize::from(!user.contains_key("whitelist"))
        );

        // The person's prefixes, then the roots they lacked, each once.
        let theirs = prefixes(&user);
        let mut expected = theirs.clone();
        for root in &roots {
            let root = root.display().to_string();
            if !expected.contains(&root) {
                expected.push(root);
            }
        }
        assert_eq!(prefixes(&out), expected);

        // Idempotent.
        let again: Table =
            merged(Some(&merged_text), &roots).unwrap().parse().unwrap();
        assert_eq!(again, out);
    }

    #[test]
    fn no_configuration_gives_the_whitelist_alone() {
        let text = merged(None, &[PathBuf::from("/tau/a")]).unwrap();
        let out: Table = text.parse().unwrap();
        assert_eq!(prefixes(&out), ["/tau/a"]);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn a_configuration_that_does_not_parse_is_refused() {
        let error = merged(Some("[global"), &[]).unwrap_err();
        assert!(error.starts_with("direnv.toml does not parse: "), "{error}");
        assert_eq!(
            merged(Some("whitelist = 1"), &[]).unwrap_err(),
            "direnv.toml's `whitelist` is not a table"
        );
        assert_eq!(
            merged(Some("[whitelist]\nprefix = \"/a\""), &[]).unwrap_err(),
            "direnv.toml's `whitelist.prefix` is not an array"
        );
    }

    /// tau's directory gets the merged file and links to the person's
    /// `lib` and `direnvrc`, or none where they have none.
    #[test]
    fn writing_links_the_persons_extensions() {
        let user = tempfile::tempdir().unwrap();
        std::fs::write(
            user.path().join("direnv.toml"),
            "[global]\nstrict_env = true\n",
        )
        .unwrap();
        std::fs::create_dir(user.path().join("lib")).unwrap();
        let ours = tempfile::tempdir().unwrap();
        let dir = ours.path().join("config");
        write(&dir, Some(user.path()), &[PathBuf::from("/tau/a")]).unwrap();
        let out: Table = std::fs::read_to_string(dir.join("direnv.toml"))
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(out["global"]["strict_env"], Value::Boolean(true));
        assert_eq!(prefixes(&out), ["/tau/a"]);
        assert_eq!(
            std::fs::read_link(dir.join("lib")).unwrap(),
            user.path().join("lib")
        );
        assert!(!dir.join("direnvrc").exists());
        // Written again with nobody allowed: the whitelist is empty.
        write(&dir, Some(user.path()), &[]).unwrap();
        let out: Table = std::fs::read_to_string(dir.join("direnv.toml"))
            .unwrap()
            .parse()
            .unwrap();
        assert!(prefixes(&out).is_empty());
    }
}
