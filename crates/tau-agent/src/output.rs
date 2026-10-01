//! Tool output too large for the model: what it costs in tokens, cut in
//! the middle to fit, and kept whole in a file the model can read back.

use std::{
    fs::File,
    io::{self, Write as _},
    os::unix::fs::OpenOptionsExt as _,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

/// Tokens in `text`, at four characters a token, rounded up.
pub fn estimate_tokens(text: &str) -> u64 {
    (text.chars().count() as u64).div_ceil(4)
}

/// `text` cut in the middle to fit a token budget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cut<'a> {
    Whole,
    Cut {
        head: &'a str,
        tail: &'a str,
        original_tokens: u64,
        removed_tokens: u64,
    },
}

/// Cuts `text` to `max_tokens`, keeping half the budget's characters
/// from each end, at character boundaries.
pub fn cut(text: &str, max_tokens: u64) -> Cut<'_> {
    let original_tokens = estimate_tokens(text);
    if original_tokens <= max_tokens {
        return Cut::Whole;
    }
    let chars = text.chars().count();
    let keep = (max_tokens.saturating_mul(4)).min(chars as u64) as usize;
    let head_chars = keep / 2;
    let tail_chars = keep - head_chars;
    let head_end = text
        .char_indices()
        .nth(head_chars)
        .map_or(text.len(), |(at, _)| at);
    let tail_start = if tail_chars == 0 {
        text.len()
    } else {
        text.char_indices()
            .nth(chars - tail_chars)
            .map_or(text.len(), |(at, _)| at)
    };
    Cut::Cut {
        head: &text[..head_end],
        tail: &text[tail_start..],
        original_tokens,
        removed_tokens: estimate_tokens(&text[head_end..tail_start]),
    }
}

/// `full`, cut to `max_tokens` with a notice of the cut, the line count
/// and where `save` kept the whole text; `full` itself when it fits.
pub fn truncated(
    full: &str,
    max_tokens: u64,
    save: impl FnOnce(&str) -> io::Result<PathBuf>,
) -> String {
    let Cut::Cut {
        head,
        tail,
        original_tokens,
        removed_tokens,
    } = cut(full, max_tokens)
    else {
        return full.to_owned();
    };
    let lines = full.lines().count().max(1);
    let saved = match save(full) {
        Ok(path) => format!(
            "[Full output: {} (read it with offset/limit)]",
            path.display()
        ),
        Err(error) => format!("[Full output could not be saved: {error}]"),
    };
    format!(
        "Warning: truncated output (original token count: \
         {original_tokens})\nTotal output lines: {lines}\n\n{head}…\
         {removed_tokens} tokens truncated…{tail}\n\n{saved}"
    )
}

/// Where output too large for the model is kept: new files
/// `<prefix>-<hex>.<extension>` in a directory, readable by their owner
/// only, since the directory may be shared.
#[derive(Debug, Clone)]
pub struct Spill {
    dir: PathBuf,
    prefix: String,
}

impl Spill {
    pub fn new(dir: impl Into<PathBuf>, prefix: impl Into<String>) -> Self {
        Self {
            dir: dir.into(),
            prefix: prefix.into(),
        }
    }

    /// Files in `$TMPDIR`.
    pub fn temp(prefix: impl Into<String>) -> Self {
        Self::new(std::env::temp_dir(), prefix)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// A new, empty file, never one already there.
    pub fn create(&self, extension: &str) -> io::Result<(PathBuf, File)> {
        let path = self.dir.join(format!(
            "{}-{:016x}.{extension}",
            self.prefix,
            unique()
        ));
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?;
        Ok((path, file))
    }

    /// A new file holding `bytes`.
    pub fn write(&self, bytes: &[u8], extension: &str) -> io::Result<PathBuf> {
        let (path, mut file) = self.create(extension)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(path)
    }
}

/// A number no other call in this process gives, and unlikely in
/// another's: the clock, the process id and a counter, mixed.
fn unique() -> u128 {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    nanos ^ (u128::from(std::process::id()) << 64) ^ u128::from(counter)
}
