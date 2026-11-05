//! A run's title until one is written: the first line of what it is
//! about. Writing one is the `tau.title` slot's (ADR 0045; tau-titles
//! fills it), from [`task`].

pub use tau_ui_plugin::title::MAX_CHARS;

/// What a run on `prompt` is about: the goal's words for a `/goal`,
/// the prompt itself otherwise.
pub fn task(prompt: &str) -> String {
    crate::plugins::read_prompt(prompt).unwrap_or_else(|| prompt.to_owned())
}

/// What a run shows until its title is written: the first line of
/// what it is about, cut at [`MAX_CHARS`] with an ellipsis.
pub fn placeholder(prompt: &str) -> String {
    let task = task(prompt);
    let line = task
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if line.is_empty() {
        return "Untitled".into();
    }
    if line.chars().count() <= MAX_CHARS {
        return line;
    }
    let cut: String = line.chars().take(MAX_CHARS - 1).collect();
    format!("{}…", cut.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A goal's run is titled and asked about by its condition, not the
    /// command.
    #[test]
    fn a_goal_is_titled_by_its_condition() {
        let goal = "/goal --continuations 3 all tests pass";
        assert_eq!(task(goal), "all tests pass");
        assert_eq!(placeholder(goal), "all tests pass");
    }

    /// A placeholder is the first line with words, its white space made
    /// single spaces: itself up to `MAX_CHARS` characters, and past that
    /// its first `MAX_CHARS - 1` characters, trimmed, and an ellipsis.
    /// With no words at all, it is `Untitled`. Lines are drawn around
    /// the limit, in characters of several bytes.
    #[hegel::test(test_cases = 300)]
    fn placeholders_are_the_first_line(tc: hegel::TestCase) {
        use hegel::generators as gs;
        let word = || {
            gs::vecs(gs::sampled_from(vec!['a', 'Z', 'é', '語', '🦀']))
                .min_size(1)
                .max_size(9)
        };
        let gap = || gs::sampled_from(vec![" ", "  ", "\t", " \t "]);
        let lead: String = tc
            .draw(gs::sampled_from(vec!["", "\n", "  \n \n", "\t"]))
            .to_owned();
        // Words up to a line around the limit.
        let target: usize = tc.draw(hegel::one_of!(
            gs::integers::<usize>()
                .min_value(MAX_CHARS - 2)
                .max_value(MAX_CHARS + 2),
            gs::integers::<usize>().max_value(MAX_CHARS * 2),
        ));
        let mut words: Vec<String> = Vec::new();
        while words.join(" ").chars().count() < target {
            let chars: Vec<char> = tc.draw(word());
            words.push(chars.into_iter().collect());
        }
        let mut text = lead;
        for (n, word) in words.iter().enumerate() {
            if n > 0 {
                text.push_str(tc.draw(gap()));
            }
            text.push_str(word);
        }
        if !words.is_empty() && tc.draw(gs::booleans()) {
            text.push_str("\nthe next line");
        }
        let line = words.join(" ");
        let count = line.chars().count();
        let expected = if line.is_empty() {
            "Untitled".to_owned()
        } else if count <= MAX_CHARS {
            line
        } else {
            let cut: String = line.chars().take(MAX_CHARS - 1).collect();
            format!("{}…", cut.trim_end())
        };
        assert_eq!(placeholder(&text), expected, "{text:?}");
    }
}
