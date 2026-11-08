//! The `tau-plugins` skill tau ships (ADR 0027): how to write, test and
//! land a plugin. Its six worked plugins are files of their own, run
//! as tests of the host, and inlined into `SKILL.md` as it is installed,
//! so the skill cannot drift from the interface.

use std::path::Path;

/// The skill's name, and its folder's.
pub const NAME: &str = crate::SKILL;

const TEMPLATE: &str = include_str!("../skill/SKILL.md");

/// The worked plugins: each one's name and files, by their paths in its
/// folder.
pub const EXAMPLES: [(&str, &[(&str, &str)]); 6] = [
    (
        "no-friday-deploys",
        &[
            (
                "tau.toml",
                include_str!("../skill/examples/no-friday-deploys/tau.toml"),
            ),
            (
                "src/plugin.luau",
                include_str!(
                    "../skill/examples/no-friday-deploys/src/plugin.luau"
                ),
            ),
            (
                "tests/rule.luau",
                include_str!(
                    "../skill/examples/no-friday-deploys/tests/rule.luau"
                ),
            ),
        ],
    ),
    (
        "word-count",
        &[
            (
                "tau.toml",
                include_str!("../skill/examples/word-count/tau.toml"),
            ),
            (
                "src/plugin.luau",
                include_str!("../skill/examples/word-count/src/plugin.luau"),
            ),
            (
                "src/words.luau",
                include_str!("../skill/examples/word-count/src/words.luau"),
            ),
            (
                "tests/tool.luau",
                include_str!("../skill/examples/word-count/tests/tool.luau"),
            ),
        ],
    ),
    (
        "say-the-tests",
        &[
            (
                "tau.toml",
                include_str!("../skill/examples/say-the-tests/tau.toml"),
            ),
            (
                "src/plugin.luau",
                include_str!("../skill/examples/say-the-tests/src/plugin.luau"),
            ),
            (
                "tests/stop.luau",
                include_str!("../skill/examples/say-the-tests/tests/stop.luau"),
            ),
        ],
    ),
    (
        "forget-old-calls",
        &[
            (
                "tau.toml",
                include_str!("../skill/examples/forget-old-calls/tau.toml"),
            ),
            (
                "src/plugin.luau",
                include_str!(
                    "../skill/examples/forget-old-calls/src/plugin.luau"
                ),
            ),
            (
                "tests/rewrite.luau",
                include_str!(
                    "../skill/examples/forget-old-calls/tests/rewrite.luau"
                ),
            ),
        ],
    ),
    (
        "short-titles",
        &[
            (
                "tau.toml",
                include_str!("../skill/examples/short-titles/tau.toml"),
            ),
            (
                "src/plugin.luau",
                include_str!("../skill/examples/short-titles/src/plugin.luau"),
            ),
            (
                "tests/title.luau",
                include_str!("../skill/examples/short-titles/tests/title.luau"),
            ),
        ],
    ),
    (
        "todo-page",
        &[
            (
                "tau.toml",
                include_str!("../skill/examples/todo-page/tau.toml"),
            ),
            (
                "src/plugin.luau",
                include_str!("../skill/examples/todo-page/src/plugin.luau"),
            ),
            (
                "tests/page.luau",
                include_str!("../skill/examples/todo-page/tests/page.luau"),
            ),
        ],
    ),
];

/// `SKILL.md`, each `{{example}}` replaced by that plugin's files.
pub fn render() -> String {
    let mut text = TEMPLATE.to_owned();
    for (name, files) in EXAMPLES {
        let shown: Vec<String> = files
            .iter()
            .map(|(path, source)| {
                format!(
                    "`{name}/{path}`:\n\n```{}\n{}\n```",
                    if path.ends_with(".toml") {
                        "toml"
                    } else {
                        "luau"
                    },
                    source.trim_end()
                )
            })
            .collect();
        text = text.replace(&format!("{{{{{name}}}}}"), &shown.join("\n\n"));
    }
    text
}

/// Writes the skill into `dir/tau-plugins`: `SKILL.md`, and the worked
/// plugins under `examples/` to copy from. What was there is replaced.
/// It blocks: call it from `spawn_blocking`.
pub fn install(dir: &Path) -> std::io::Result<()> {
    let root = dir.join(NAME);
    if root.exists() {
        std::fs::remove_dir_all(&root)?;
    }
    std::fs::create_dir_all(&root)?;
    std::fs::write(root.join("SKILL.md"), render())?;
    for (name, files) in EXAMPLES {
        for (path, source) in files {
            let file = root.join("examples").join(name).join(path);
            if let Some(parent) = file.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(file, source)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_example_is_inlined() {
        let text = render();
        assert!(!text.contains("{{"), "{text}");
        for (name, files) in EXAMPLES {
            for (path, source) in files {
                assert!(text.contains(&format!("`{name}/{path}`")));
                assert!(text.contains(source.trim_end()));
            }
        }
    }
}
