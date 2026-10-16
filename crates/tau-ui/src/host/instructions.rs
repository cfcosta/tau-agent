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
    /// fits on a character's boundary, and says it was cut. Files are a
    /// short pattern of one- to four-byte characters repeated to a length
    /// drawn around the limit, so the limit falls on every byte of a
    /// character, and a file of exactly the limit is drawn too.
    #[hegel::test(test_cases = 300)]
    fn the_section_keeps_what_fits(tc: hegel::TestCase) {
        let pattern: Vec<char> = tc.draw(
            gs::vecs(gs::sampled_from(vec!['a', 'é', '語', '🦀']))
                .min_size(1)
                .max_size(6),
        );
        let target: usize = tc.draw(hegel::one_of!(
            gs::integers::<usize>()
                .min_value(AGENTS_LIMIT - 4)
                .max_value(AGENTS_LIMIT + 4),
            gs::integers::<usize>().max_value(AGENTS_LIMIT + 64),
        ));
        let mut text = String::new();
        for ch in pattern.iter().cycle() {
            if text.len() >= target {
                break;
            }
            text.push(*ch);
        }
        // Exactly the limit, when the pattern lets the length land there.
        if target == AGENTS_LIMIT && text.len() > AGENTS_LIMIT {
            while text.len() > AGENTS_LIMIT - 4 {
                text.pop();
            }
            while text.len() < AGENTS_LIMIT {
                text.push('a');
            }
        }
        let section = agents_section(text.as_bytes());
        let head = agents_section(b"");
        assert!(head.starts_with(AGENTS_HEADING));
        if text.len() > AGENTS_LIMIT {
            let kept = text
                .char_indices()
                .map(|(at, c)| at + c.len_utf8())
                .take_while(|end| *end <= AGENTS_LIMIT)
                .last()
                .unwrap_or(0);
            let note = section
                .strip_prefix(&format!("{head}{}", &text[..kept]))
                .unwrap_or_else(|| {
                    panic!("not the kept prefix: {}", text.len())
                });
            assert!(note.starts_with("\n\n[AGENTS.md was cut here"), "{note}");
        } else {
            assert_eq!(section, format!("{head}{text}"), "{}", text.len());
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
