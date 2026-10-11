//! The `tau-agents` skill tau ships (ADR 0037): how an agent writes,
//! tests and changes itself. Its worked agents are files of their own,
//! run as tests of the host, and inlined into `SKILL.md` as it is
//! installed, so the skill cannot drift from what the host reads.

use std::path::Path;

/// The skill's name, and its folder's.
pub const NAME: &str = crate::SKILL;

const TEMPLATE: &str = include_str!("../skill/SKILL.md");

/// The worked agents: each one's name and files, by their paths in its
/// repository.
pub const EXAMPLES: [(&str, &[(&str, &str)]); 4] = [
    (
        "morning-digest",
        &[
            (
                "agent.luau",
                include_str!("../skill/examples/morning-digest/agent.luau"),
            ),
            (
                "tests/triggers.luau",
                include_str!(
                    "../skill/examples/morning-digest/tests/triggers.luau"
                ),
            ),
        ],
    ),
    (
        "release-watch",
        &[
            (
                "agent.luau",
                include_str!("../skill/examples/release-watch/agent.luau"),
            ),
            (
                "lib/releases.luau",
                include_str!(
                    "../skill/examples/release-watch/lib/releases.luau"
                ),
            ),
            (
                "tests/poll.luau",
                include_str!("../skill/examples/release-watch/tests/poll.luau"),
            ),
        ],
    ),
    (
        "build-watch",
        &[
            (
                "agent.luau",
                include_str!("../skill/examples/build-watch/agent.luau"),
            ),
            (
                "tests/parse.luau",
                include_str!("../skill/examples/build-watch/tests/parse.luau"),
            ),
        ],
    ),
    (
        "review-desk",
        &[
            (
                "agent.luau",
                include_str!("../skill/examples/review-desk/agent.luau"),
            ),
            (
                "tests/triggers.luau",
                include_str!(
                    "../skill/examples/review-desk/tests/triggers.luau"
                ),
            ),
        ],
    ),
];

/// `SKILL.md`, each `{{example}}` replaced by that agent's files.
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

/// Writes the skill into `dir/tau-agents`: `SKILL.md`, and the worked
/// agents under `examples/` to copy from. What was there is replaced.
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
