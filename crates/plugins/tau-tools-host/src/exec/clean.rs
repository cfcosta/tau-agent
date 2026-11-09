//! A process's terminal output as the plain text the model reads, kept
//! as it arrives.
//!
//! The bytes a program writes to its terminal hold escape sequences,
//! carriage returns that redraw a line, and backspaces. [`Cleaner`]
//! applies the few of those that change what a line says (`\r`, `\b`,
//! tabs, erase in line, cursor to column) and drops the rest, so a
//! progress bar leaves its last frame and colors leave no trace. Lines
//! that ended are final; the line still being written is [`Cleaner::take`]n
//! as it is, and what was said of it already is not said again.

use std::collections::VecDeque;

/// The most plain text kept unread, in bytes; the oldest lines go
/// first.
pub const MAX_UNREAD: usize = 1 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Text,
    /// After `ESC`.
    Escape,
    /// After `ESC [`: parameters, then a final byte.
    Csi,
    /// After `ESC ]`, `ESC P`, `ESC X`, `ESC ^` or `ESC _`: a string up
    /// to `BEL` or `ESC \`.
    Str,
    /// `ESC` inside a string: `\` ends it.
    StrEscape,
    /// After `ESC (` and its kind: one more character.
    Charset,
}

/// Plain text from terminal output; see the module's docs.
#[derive(Debug)]
pub struct Cleaner {
    mode: Mode,
    /// A `UTF-8` sequence cut by the end of a chunk.
    carry: Vec<u8>,
    params: String,
    /// The line being written, by column.
    line: Vec<char>,
    col: usize,
    /// Lines that ended and were not taken.
    done: VecDeque<String>,
    unread: usize,
    /// Lines dropped from `done` since the last take, past
    /// [`MAX_UNREAD`].
    dropped: usize,
    /// What the last take said of the line being written.
    said: Option<String>,
    /// What the next take's first line goes on from it with, when it does.
    joins: Option<String>,
}

/// What was read since the last [`Cleaner::take`].
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Taken {
    pub text: String,
    /// It holds a line, which may be empty.
    pub any: bool,
    /// Lines that were dropped for want of room before being read.
    pub dropped: usize,
    /// The last line was still being written.
    pub open: bool,
    /// The first line goes on from the line the last take left open,
    /// after this whitespace.
    pub joins: Option<String>,
}

impl Taken {
    /// Adds what a later take found: a line one take left open goes on
    /// in the next, as it was written.
    pub fn merge(&mut self, later: Taken) {
        if later.any {
            if self.any {
                match later.joins.as_deref().filter(|_| self.open) {
                    Some(gap) => self.text.push_str(gap),
                    None => self.text.push('\n'),
                }
            }
            self.text.push_str(&later.text);
            self.any = true;
            self.open = later.open;
        }
        self.dropped += later.dropped;
    }
}

/// `rest` without its leading whitespace, and that whitespace.
fn split_gap(rest: &str) -> (&str, &str) {
    let text = rest.trim_start();
    (text, &rest[..rest.len() - text.len()])
}

impl Default for Cleaner {
    fn default() -> Self {
        Self {
            mode: Mode::Text,
            carry: Vec::new(),
            params: String::new(),
            line: Vec::new(),
            col: 0,
            done: VecDeque::new(),
            unread: 0,
            dropped: 0,
            said: None,
            joins: None,
        }
    }
}

impl Cleaner {
    /// Reads `bytes` of terminal output.
    pub fn feed(&mut self, bytes: &[u8]) {
        let mut input = std::mem::take(&mut self.carry);
        input.extend_from_slice(bytes);
        let mut rest = input.as_slice();
        loop {
            match std::str::from_utf8(rest) {
                Ok(text) => {
                    text.chars().for_each(|c| self.push(c));
                    return;
                }
                Err(error) => {
                    let (good, bad) = rest.split_at(error.valid_up_to());
                    // SAFETY-free: `valid_up_to` is a boundary.
                    std::str::from_utf8(good)
                        .expect("valid up to here")
                        .chars()
                        .for_each(|c| self.push(c));
                    match error.error_len() {
                        // A sequence the chunk cut: the next feed has
                        // the rest.
                        None => {
                            self.carry = bad.to_vec();
                            return;
                        }
                        Some(len) => {
                            self.push('\u{fffd}');
                            rest = &bad[len..];
                        }
                    }
                }
            }
        }
    }

    fn push(&mut self, c: char) {
        match self.mode {
            Mode::Text => self.text(c),
            Mode::Escape => match c {
                '[' => {
                    self.params.clear();
                    self.mode = Mode::Csi;
                }
                ']' | 'P' | 'X' | '^' | '_' => self.mode = Mode::Str,
                '(' | ')' | '*' | '+' | '-' | '.' | '/' | '#' | '%' => {
                    self.mode = Mode::Charset
                }
                _ => self.mode = Mode::Text,
            },
            Mode::Csi => {
                if ('\u{40}'..='\u{7e}').contains(&c) {
                    self.csi(c);
                    self.mode = Mode::Text;
                } else {
                    self.params.push(c);
                }
            }
            Mode::Str => match c {
                '\u{7}' => self.mode = Mode::Text,
                '\u{1b}' => self.mode = Mode::StrEscape,
                _ => {}
            },
            Mode::StrEscape => {
                self.mode = if c == '\\' { Mode::Text } else { Mode::Str }
            }
            Mode::Charset => self.mode = Mode::Text,
        }
    }

    fn text(&mut self, c: char) {
        match c {
            '\u{1b}' => self.mode = Mode::Escape,
            '\n' => self.end_line(),
            '\r' => self.col = 0,
            '\u{8}' => self.col = self.col.saturating_sub(1),
            // Only the cursor moves: what is skipped stays.
            '\t' => self.col = (self.col / 8 + 1) * 8,
            c if c.is_control() => {}
            c => self.write(c),
        }
    }

    fn write(&mut self, c: char) {
        while self.line.len() < self.col {
            self.line.push(' ');
        }
        if self.col < self.line.len() {
            self.line[self.col] = c;
        } else {
            self.line.push(c);
        }
        self.col += 1;
    }

    fn csi(&mut self, last: char) {
        let n = || {
            self.params
                .split(';')
                .next()
                .and_then(|p| p.parse::<usize>().ok())
        };
        match last {
            // Erase in line: to the end, to the start, all.
            'K' => match n().unwrap_or(0) {
                0 => self.line.truncate(self.col),
                1 => {
                    let to = self.col.min(self.line.len());
                    self.line[..to].fill(' ');
                }
                _ => self.line.clear(),
            },
            // Cursor to column, one-based.
            'G' => self.col = n().unwrap_or(1).max(1) - 1,
            // Cursor left.
            'D' => self.col = self.col.saturating_sub(n().unwrap_or(1).max(1)),
            // Cursor right.
            'C' => self.col += n().unwrap_or(1).max(1),
            _ => {}
        }
    }

    fn end_line(&mut self) {
        let line: String = self.line.drain(..).collect();
        self.col = 0;
        let mut line = line.trim_end().to_owned();
        // The part of it that was said already, as it was written.
        if let Some(said) = self.said.take() {
            let said = said.trim_end();
            if let Some(rest) = line.strip_prefix(said) {
                let (text, gap) = split_gap(rest);
                let (text, gap) = (text.to_owned(), gap.to_owned());
                line = text;
                if line.is_empty() {
                    return;
                }
                self.joins = Some(gap);
            }
        }
        self.unread += line.len() + 1;
        self.done.push_back(line);
        while self.unread > MAX_UNREAD {
            let Some(dropped) = self.done.pop_front() else {
                break;
            };
            self.unread -= dropped.len() + 1;
            self.dropped += 1;
            self.joins = None;
        }
    }

    /// Whether anything unread is waiting.
    pub fn has_unread(&self) -> bool {
        self.dropped > 0 || !self.done.is_empty() || self.partial().is_some()
    }

    /// The line being written, when it says something not said yet.
    fn partial(&self) -> Option<String> {
        let line: String = self.line.iter().collect();
        let line = line.trim_end();
        if line.is_empty() {
            return None;
        }
        match &self.said {
            Some(said) if said.trim_end() == line => None,
            Some(said) => match line.strip_prefix(said.trim_end()) {
                Some(rest) => Some(rest.trim_start().to_owned())
                    .filter(|rest| !rest.is_empty()),
                None => Some(line.to_owned()),
            },
            None => Some(line.to_owned()),
        }
    }

    /// Puts back text a take handed over and nobody read: it is first
    /// in the next one.
    pub fn give_back(&mut self, text: String) {
        if text.is_empty() {
            return;
        }
        self.said = None;
        self.joins = None;
        for line in text.split('\n').rev() {
            self.unread += line.len() + 1;
            self.done.push_front(line.to_owned());
        }
    }

    /// What was written since the last take, as lines; the line still
    /// being written comes as far as it is, and is not said again.
    pub fn take(&mut self) -> Taken {
        let partial = self.partial();
        let line: String = self.line.iter().collect();
        let mut lines: Vec<String> = self.done.drain(..).collect();
        self.unread = 0;
        let mut joins = self.joins.take();
        if lines.is_empty() && partial.is_some() {
            joins = self.said.as_deref().and_then(|said| {
                let rest = line.trim_end().strip_prefix(said.trim_end())?;
                Some(split_gap(rest).1.to_owned())
            });
        }
        let open = partial.is_some();
        if let Some(partial) = partial {
            lines.push(partial);
            self.said = Some(line);
        }
        Taken {
            text: lines.join("\n"),
            any: !lines.is_empty(),
            dropped: std::mem::take(&mut self.dropped),
            open,
            joins,
        }
    }
}

#[cfg(test)]
mod tests {
    use hegel::{TestCase, generators as gs};

    use super::*;

    /// Property inventory: the text read is the same however the bytes
    /// are cut into chunks, and a take between chunks, joined, says
    /// the same as one take at the end once every line ended.
    /// Generator: pieces of terminal output (text, a multi-byte
    /// character, line ends, redraws, backspace, erase in line, color,
    /// a title), cut at arbitrary points, including inside a character or
    /// an escape sequence. The workspace hegel.toml supplies case counts.
    #[hegel::test]
    fn the_text_does_not_depend_on_how_the_bytes_are_cut(tc: TestCase) {
        let pieces: Vec<&[u8]> = tc.draw(
            gs::vecs(gs::sampled_from(vec![
                "abc".as_bytes(),
                "é世".as_bytes(),
                b"\r\n",
                b"\n",
                b"\r",
                b"\x08",
                b"\t",
                b"\x1b[K",
                b"\x1b[2K",
                b"\x1b[31m",
                b"\x1b[0m",
                b"\x1b]0;title\x07",
                b"\x1b(B",
                b"x",
            ]))
            .max_size(40),
        );
        let mut bytes: Vec<u8> = pieces.concat();
        // Every line ends, so what a take said of a line half written
        // does not matter.
        bytes.extend_from_slice(b"\r\n");
        let mut cuts: Vec<usize> = tc.draw(
            gs::vecs(
                gs::integers::<usize>().min_value(0).max_value(bytes.len()),
            )
            .max_size(10),
        );
        cuts.sort_unstable();
        let mut whole = Cleaner::default();
        whole.feed(&bytes);
        let mut cut = Cleaner::default();
        let mut start = 0;
        for end in cuts.into_iter().chain([bytes.len()]) {
            cut.feed(&bytes[start..end]);
            start = end;
        }
        assert_eq!(cut.take().text, whole.take().text);
    }

    /// Property inventory: takes made anywhere in what a process writes,
    /// merged, are the text of one take at the end, so a read that lands
    /// inside a line does not break it.
    /// Generator: text, spaces and line ends, without redraws, which say
    /// a line again on purpose, cut at arbitrary points.
    #[hegel::test]
    fn takes_merged_are_one_take(tc: TestCase) {
        let pieces: Vec<&[u8]> = tc.draw(
            gs::vecs(gs::sampled_from(vec![
                "abc".as_bytes(),
                "é世".as_bytes(),
                b" ",
                b"  ",
                b"\t",
                b"\r\n",
                b"\n",
                b"\x1b[31m",
                b"x",
            ]))
            .max_size(40),
        );
        let bytes = pieces.concat();
        let mut cuts: Vec<usize> = tc.draw(
            gs::vecs(
                gs::integers::<usize>().min_value(0).max_value(bytes.len()),
            )
            .max_size(10),
        );
        cuts.sort_unstable();
        let mut whole = Cleaner::default();
        whole.feed(&bytes);
        let mut cut = Cleaner::default();
        let mut merged = Taken::default();
        let mut start = 0;
        for end in cuts.into_iter().chain([bytes.len()]) {
            cut.feed(&bytes[start..end]);
            merged.merge(cut.take());
            start = end;
        }
        assert_eq!(merged.text, whole.take().text);
    }

    /// Shrunk from a run under load: a read landed inside `line 1002`,
    /// and the model read it as two lines.
    #[test]
    fn a_read_inside_a_line_does_not_break_it() {
        let mut cleaner = Cleaner::default();
        let mut merged = Taken::default();
        cleaner.feed(b"line 1001\nline 1");
        merged.merge(cleaner.take());
        cleaner.feed(b"002\nline 1003\n");
        merged.merge(cleaner.take());
        assert_eq!(merged.text, "line 1001\nline 1002\nline 1003");
    }

    fn text(chunks: &[&[u8]]) -> String {
        let mut cleaner = Cleaner::default();
        chunks.iter().for_each(|chunk| cleaner.feed(chunk));
        cleaner.take().text
    }

    #[test]
    fn lines_come_out_without_the_terminals_carriage_returns() {
        assert_eq!(text(&[b"one\r\ntwo\r\n"]), "one\ntwo");
    }

    #[test]
    fn colors_leave_no_trace() {
        assert_eq!(text(&[b"\x1b[31mred\x1b[0m plain\r\n"]), "red plain");
    }

    #[test]
    fn a_progress_bar_leaves_its_last_frame() {
        assert_eq!(text(&[b"10%\r50%\r100%\r\ndone\r\n"]), "100%\ndone");
        assert_eq!(text(&[b"long line\r\x1b[Kshort\r\n"]), "short");
    }

    #[test]
    fn titles_and_charsets_are_dropped() {
        assert_eq!(
            text(&[b"\x1b]0;title\x07a\x1b]8;;http://x\x1b\\b\x1b(Bc\r\n"]),
            "abc"
        );
    }

    /// A byte that is no part of UTF-8 reads as U+FFFD. (A character cut
    /// between chunks is `the_text_does_not_depend_on_how_the_bytes_are_cut`.)
    #[test]
    fn a_byte_that_is_not_utf8_is_a_replacement_character() {
        assert_eq!(text(&[b"a\xffb\r\n"]), "a\u{fffd}b");
    }

    #[test]
    fn backspace_and_tab_move_the_cursor() {
        assert_eq!(text(&[b"abc\x08\x08X\tY\r\n"]), "aXc     Y");
    }

    #[test]
    fn a_line_still_being_written_is_said_once() {
        let mut cleaner = Cleaner::default();
        cleaner.feed(b"ready? ");
        assert_eq!(cleaner.take().text, "ready?");
        assert!(!cleaner.has_unread());
        cleaner.feed(b"yes\r\nnext");
        assert_eq!(cleaner.take().text, "yes\nnext");
        cleaner.feed(b"\r\n");
        assert_eq!(cleaner.take().text, "");
        cleaner.feed(b"redrawn\rREDRAWN\r\n");
        assert_eq!(cleaner.take().text, "REDRAWN");
    }

    /// Past [`MAX_UNREAD`] unread bytes, whole lines go from the front:
    /// with lines of 1 KiB each (the newline counted), exactly the
    /// newest `MAX_UNREAD / 1024` stay, and the take says how many went.
    #[test]
    fn what_is_not_read_in_time_is_dropped_oldest_first() {
        let mut cleaner = Cleaner::default();
        // Numbered, so which lines stay shows.
        let lines: Vec<String> = (0..1100)
            .map(|i| format!("{i:04}{}", "x".repeat(1019)))
            .collect();
        for line in &lines {
            cleaner.feed(format!("{line}\r\n").as_bytes());
        }
        let taken = cleaner.take();
        let kept = MAX_UNREAD / 1024;
        assert_eq!(taken.dropped, lines.len() - kept);
        assert_eq!(taken.text, lines[lines.len() - kept..].join("\n"));
    }
}
