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
}

/// What was read since the last [`Cleaner::take`].
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Taken {
    pub text: String,
    /// Lines that were dropped for want of room before being read.
    pub dropped: usize,
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
                line = rest.trim_start().to_owned();
                if line.is_empty() {
                    return;
                }
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
        }
    }

    /// Whether anything unread is waiting.
    pub fn has_unread(&self) -> bool {
        self.dropped > 0
            || !self.done.is_empty()
            || self.partial().is_some()
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
        if let Some(partial) = partial {
            lines.push(partial);
            self.said = Some(line);
        }
        Taken {
            text: lines.join("\n"),
            dropped: std::mem::take(&mut self.dropped),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(
            text(&[b"10%\r50%\r100%\r\ndone\r\n"]),
            "100%\ndone"
        );
        assert_eq!(text(&[b"long line\r\x1b[Kshort\r\n"]), "short");
    }

    #[test]
    fn titles_and_charsets_are_dropped() {
        assert_eq!(
            text(&[b"\x1b]0;title\x07a\x1b]8;;http://x\x1b\\b\x1b(Bc\r\n"]),
            "abc"
        );
    }

    #[test]
    fn a_character_cut_between_chunks_is_whole() {
        let bytes = "é\r\n".as_bytes();
        assert_eq!(text(&[&bytes[..1], &bytes[1..]]), "é");
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

    #[test]
    fn what_is_not_read_in_time_is_dropped_oldest_first() {
        let mut cleaner = Cleaner::default();
        let line = format!("{}\r\n", "x".repeat(1023));
        for _ in 0..1100 {
            cleaner.feed(line.as_bytes());
        }
        let taken = cleaner.take();
        assert!(taken.dropped > 0 && taken.dropped < 100, "{}", taken.dropped);
        assert!(taken.text.len() <= MAX_UNREAD + 1024);
    }
}
