//! The `tau-plugins` skill tau ships (ADR 0027): how to write, test and
//! land a plugin. Its three worked plugins are files of their own, run
//! as tests of the host, and inlined into `SKILL.md` as it is installed,
//! so the skill cannot drift from the interface.

use std::path::Path;

/// The skill's name, and its folder's.
pub const NAME: &str = "tau-plugins";

const TEMPLATE: &str = include_str!("../skill/SKILL.md");

/// The worked plugins: each one's name and files, by their paths in its
/// folder.
pub const EXAMPLES: [(&str, &[(&str, &str)]); 3] = [
    (
        "no-friday-deploys",
        &[
            (
                "plugin.luau",
                include_str!("../skill/examples/no-friday-deploys/plugin.luau"),
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
                "plugin.luau",
                include_str!("../skill/examples/word-count/plugin.luau"),
            ),
            (
                "lib/words.luau",
                include_str!("../skill/examples/word-count/lib/words.luau"),
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
                "plugin.luau",
                include_str!("../skill/examples/say-the-tests/plugin.luau"),
            ),
            (
                "tests/stop.luau",
                include_str!("../skill/examples/say-the-tests/tests/stop.luau"),
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
                    "`{name}/{path}`:\n\n```luau\n{}\n```",
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
