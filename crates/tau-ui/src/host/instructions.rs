//! The repository's own instructions: `AGENTS.md` at the root of the
//! workspace a run works in, added to the run's instructions as it
//! starts (ADR 0020).

use super::*;

/// The file a repository keeps its instructions for agents in.
pub const AGENTS_FILE: &str = "AGENTS.md";

/// How much of [`AGENTS_FILE`] a run takes, in bytes. A longer file is
/// cut, and the instructions say so.
pub const AGENTS_LIMIT: usize = 32 * 1024;

/// The heading the file's text goes under in the instructions.
pub const AGENTS_HEADING: &str = "# The repository's instructions";

/// Adds the workspace's [`AGENTS_FILE`] to a run's instructions as it
/// starts. It is read at each start, not each turn: the instructions
/// are fixed for a run's session (the WebSocket's delta rule), and a
/// chat that edits the file sees its own version the next time it
/// starts. A missing or unreadable file adds nothing.
pub(super) struct RepoInstructions {
    pub(super) dir: PathBuf,
}

#[async_trait]
impl Plugin for RepoInstructions {
    fn name(&self) -> &str {
        "tau-agents-file"
    }

    async fn start(
        &self,
        plan: &mut RunPlan,
        _ctx: &PluginCtx,
    ) -> Result<Box<dyn PluginRun>, PluginError> {
        let path = self.dir.join(AGENTS_FILE);
        let read = tokio::task::spawn_blocking(move || std::fs::read(path))
            .await
            .ok()
            .and_then(Result::ok);
        if let Some(bytes) = read {
            let section = agents_section(&bytes);
            plan.instructions = Some(match plan.instructions.take() {
                Some(base) => format!("{base}\n\n{section}"),
                None => section,
            });
        }
        Ok(Box::new(Quiet))
    }
}

/// A plugin run that does nothing after its start.
struct Quiet;

impl PluginRun for Quiet {}

/// The instructions' section for an [`AGENTS_FILE`] that holds `bytes`:
/// the heading, a line saying where it comes from, and the text, cut at
/// [`AGENTS_LIMIT`] on a character's boundary with a note when longer.
pub fn agents_section(bytes: &[u8]) -> String {
    let cut = bytes.len() > AGENTS_LIMIT;
    let kept = &bytes[..bytes.len().min(AGENTS_LIMIT)];
    // A cut can split a character: what is left of it goes.
    let text = match std::str::from_utf8(kept) {
        Ok(text) => text.to_owned(),
        Err(error) if cut && error.error_len().is_none() => {
            String::from_utf8_lossy(&kept[..error.valid_up_to()]).into_owned()
        }
        Err(_) => String::from_utf8_lossy(kept).into_owned(),
    };
    let mut section = format!(
        "{AGENTS_HEADING}\n\nThese are the instructions of the repository \
         you work in, from {AGENTS_FILE} at its root. Follow them.\n\n{}",
        text.trim_end()
    );
    if cut {
        section.push_str(&format!(
            "\n\n[{AGENTS_FILE} was cut here: it is longer than {} KiB. \
             Read the file for the rest.]",
            AGENTS_LIMIT / 1024
        ));
    }
    section
}

#[cfg(test)]
mod tests {
    use hegel::generators as gs;

    use super::*;

    /// A file within the limit comes back whole, under the heading, and
    /// says nothing of a cut; a longer one keeps the longest prefix that
    /// fits on a character's boundary, and says it was cut.
    #[hegel::test(test_cases = 300)]
    fn the_section_keeps_what_fits(tc: hegel::TestCase) {
        let unit: String = tc.draw(gs::sampled_from(vec![
            "a".to_owned(),
            "é".to_owned(),
            "語".to_owned(),
            "🦀".to_owned(),
        ]));
        let count: usize = tc.draw(gs::integers().max_value(AGENTS_LIMIT / 2));
        let text = format!("x{}", unit.repeat(count));
        let section = agents_section(text.as_bytes());
        assert!(section.starts_with(AGENTS_HEADING));
        let cut = text.len() > AGENTS_LIMIT;
        assert_eq!(section.contains("was cut here"), cut, "{}", text.len());
        if cut {
            let mut end = AGENTS_LIMIT;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            assert!(section.contains(&text[..end]));
            assert!(!section.contains(&text[..end + unit.len()]));
        } else {
            assert!(section.ends_with(&text));
        }
    }

    #[test]
    fn the_section_names_the_file() {
        let section = agents_section(b"Run `cargo test`.\n");
        assert_eq!(
            section,
            "# The repository's instructions\n\nThese are the instructions \
             of the repository you work in, from AGENTS.md at its root. \
             Follow them.\n\nRun `cargo test`."
        );
    }
}
