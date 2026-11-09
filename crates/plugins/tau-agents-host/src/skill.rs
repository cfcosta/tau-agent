//! The `tau-agents` skill tau ships (ADR 0037): how an agent writes,
//! tests and changes itself. Its worked agents are files of their own,
//! run as tests of the host, and inlined into `SKILL.md` as it is
//! installed, so the skill cannot drift from what the host reads.

use std::path::Path;

use tau_luau_plugins::packages::{self, Example};

/// The skill's name, and its folder's.
pub const NAME: &str = crate::SKILL;

const TEMPLATE: &str = include_str!("../skill/SKILL.md");

/// The worked agents, by their folders under `plugins/examples` (ADR 0051).
/// They are read as the skill is, so editing one compiles nothing.
pub const EXAMPLES: [&str; 5] = [
    "morning-digest",
    "release-watch",
    "build-watch",
    "review-desk",
    "main-status",
];

/// The worked agents' files, as this install has them.
pub fn examples() -> std::io::Result<Vec<Example>> {
    EXAMPLES
        .iter()
        .map(|name| packages::example(name).map_err(std::io::Error::other))
        .collect()
}

/// `SKILL.md`, each `{{example}}` replaced by that agent's files.
pub fn render() -> std::io::Result<String> {
    Ok(rendered(&examples()?))
}

fn rendered(examples: &[Example]) -> String {
    let mut text = TEMPLATE.to_owned();
    for Example { name, files } in examples {
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

/// Writes the skill into `dir/tau-agents`: `SKILL.md`, and the worked
/// agents under `examples/` to copy from. What was there is replaced.
/// It blocks: call it from `spawn_blocking`.
pub fn install(dir: &Path) -> std::io::Result<()> {
    // Read first: a missing folder leaves the last install as it was.
    let examples = examples()?;
    let root = dir.join(NAME);
    if root.exists() {
        std::fs::remove_dir_all(&root)?;
    }
    std::fs::create_dir_all(&root)?;
    std::fs::write(root.join("SKILL.md"), rendered(&examples))?;
    for Example { name, files } in &examples {
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
        let text = render().unwrap();
        assert!(!text.contains("{{"), "{text}");
        for Example { name, files } in examples().unwrap() {
            for (path, source) in &files {
                assert!(text.contains(&format!("`{name}/{path}`")));
                assert!(text.contains(source.trim_end()));
            }
        }
    }
}
