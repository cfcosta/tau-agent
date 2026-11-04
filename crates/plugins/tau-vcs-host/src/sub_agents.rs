//! What a caller reads about its sub-agents in jj's words: the notes
//! after a sub-agent's answer ([`landing_note`], [`kept_note`]), and
//! what `spawn` says of the caller's commits ([`spawn_notes`]). The
//! sub-agents themselves are the kernel's
//! (`tau_ui_plugin::sub_agents`).

use tau_agent::event::LimitKind;
use tau_ui_plugin::vcs::SpawnNotes;

use crate::Landing;

/// What `spawn`'s description says in jj's words.
pub fn spawn_notes() -> SpawnNotes {
    SpawnNotes {
        commit_first: "commit your work with `vcs_commit` first".into(),
        kept: "the report names its changes, for you to read with \
               `vcs_show` and take files from with `vcs_restore` `from`; \
               tau drops them when it next starts"
            .into(),
    }
}

/// What the caller reads about a landing, after the sub-agent's answer:
/// whether a limit cut the sub-agent short, how many changes landed, and
/// the paths this landing left in conflict (`brought`), not those the
/// caller's head held already.
pub fn landing_note(
    landing: &Landing,
    brought: &[String],
    limit: Option<LimitKind>,
) -> String {
    let short = cut_short(limit);
    format!("[{short}{}]", landed_note(landing, brought))
}

/// What a note says first when `limit` cut the sub-agent short.
fn cut_short(limit: Option<LimitKind>) -> &'static str {
    match limit {
        Some(LimitKind::Turns) => "It stopped at its turn limit. ",
        Some(LimitKind::Tokens) => "It stopped at its token limit. ",
        Some(LimitKind::Usd) => "It stopped at its cost limit. ",
        Some(LimitKind::Time) => "It stopped at its time limit. ",
        None => "",
    }
}

/// What the caller reads about a sub-agent kept off its work, after its
/// answer: whether a limit cut it short, and its changes, to read and
/// take files from.
pub fn kept_note(changes: &[String], limit: Option<LimitKind>) -> String {
    let short = cut_short(limit);
    let kept = match changes {
        [] => "It changed no files.".to_owned(),
        [one] => format!(
            "Its change {one} was kept beside yours, not landed: read it \
             with `vcs_show`, and take files from it with `vcs_restore` \
             `from`. tau drops it when it next starts."
        ),
        many => format!(
            "Its {} changes were kept beside yours, not landed, oldest \
             first: {}. The last holds all its files: read them with \
             `vcs_show`, and take files with `vcs_restore` `from` it. tau \
             drops them when it next starts.",
            many.len(),
            many.join(", ")
        ),
    };
    format!("[{short}{kept}]")
}

fn landed_note(landing: &Landing, brought: &[String]) -> String {
    let landed = match landing.changes.len() {
        0 => return "It changed no files.".to_owned(),
        1 => "Its 1 change landed on top of yours".to_owned(),
        n => format!("Its {n} changes landed on top of yours"),
    };
    if brought.is_empty() {
        return format!("{landed}.");
    }
    format!(
        "{landed}, with conflicts in {}: resolve their conflict markers, \
         then commit.",
        brought.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use hegel::generators as gs;

    use super::*;
    use crate::ChangeInfo;

    /// The note says which limit cut the sub-agent short, if one did, how
    /// many changes landed and, when the landing brought conflicts, names
    /// those paths and no others; a landing with nothing in it says so,
    /// whatever else it holds.
    #[hegel::test(test_cases = 200)]
    fn the_note_says_what_landed(tc: hegel::TestCase) {
        let changes: usize = tc.draw(gs::integers().max_value(5));
        let conflicts: Vec<String> = tc.draw(
            gs::vecs(gs::from_regex("[a-z]{1,6}\\.rs").fullmatch(true))
                .max_size(4)
                .unique(true),
        );
        let brought: Vec<String> = tc.draw(gs::subsequences(conflicts.clone()));
        let limits = [
            None,
            Some((LimitKind::Turns, "turn")),
            Some((LimitKind::Tokens, "token")),
            Some((LimitKind::Usd, "cost")),
            Some((LimitKind::Time, "time")),
        ];
        let limit: usize = tc.draw(gs::integers().max_value(limits.len() - 1));
        let limit = limits[limit];
        let change = |n: usize| ChangeInfo {
            change_id: format!("k{n}"),
            commit_id: format!("c{n}"),
            description: String::new(),
            empty: false,
            conflict: false,
            immutable: false,
            working_copy: false,
            divergent: false,
            bookmarks: Vec::new(),
        };
        let landing = Landing {
            changes: (0..changes).map(change).collect(),
            conflicts: conflicts.clone(),
            head: "h".into(),
        };
        let note = landing_note(&landing, &brought, limit.map(|(l, _)| l));
        // A limit that cut it short comes first.
        let note = match limit {
            Some((_, name)) => {
                let short = format!("[It stopped at its {name} limit. ");
                assert!(note.starts_with(&short), "{note}");
                format!("[{}", &note[short.len()..])
            }
            None => note,
        };
        if changes == 0 {
            assert_eq!(note, "[It changed no files.]");
            return;
        }
        let count = if changes == 1 {
            "Its 1 change landed".to_owned()
        } else {
            format!("Its {changes} changes landed")
        };
        assert!(note.starts_with(&format!("[{count}")), "{note}");
        assert_eq!(note.contains("conflicts"), !brought.is_empty(), "{note}");
        let named: Vec<&str> = note
            .split_once("with conflicts in ")
            .and_then(|(_, rest)| rest.split_once(':'))
            .map(|(list, _)| list.split(", ").collect())
            .unwrap_or_default();
        assert_eq!(named, brought, "{note}");
    }
}
