//! `grep`: search file contents for a pattern
//! (`docs/reference/tools.md`, "grep"), ported from pi's `grep.ts`.
//!
//! Search runs natively, on ripgrep's own crates: `grep-searcher` and
//! `grep-regex` do the matching, and `ignore::WalkBuilder` walks the
//! tree (hidden files included, `.gitignore` respected). There is no
//! `rg --json` subprocess. Like ripgrep, the walk and the search run on
//! every core; the output is still in path order, the same as a search
//! of one file after another (see [`search_files`]).
//!
//! **Deliberate difference from pi:** pi drops matches on lines that
//! contain U+2028 or U+2029 (`grep.ts:169`, an artifact of its `rg
//! --json` parsing). Lines here are decoded straight from the file's
//! bytes, so those matches are kept.

use std::{
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use anyhow::anyhow;
use async_trait::async_trait;
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::{
    Searcher,
    SearcherBuilder,
    Sink,
    SinkContext,
    SinkContextKind,
    SinkMatch,
};
use ignore::{
    WalkBuilder,
    WalkState,
    overrides::{Override, OverrideBuilder},
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use tau_agent::tool::{AgentTool, ToolCtx, ToolOutput};
use tokio_util::sync::CancellationToken;

use crate::{
    ABORTED,
    path::Root,
    truncate::{
        GREP_MAX_LINE,
        MAX_BYTES,
        format_size,
        truncate_head,
        truncate_line,
    },
};

const DEFAULT_LIMIT: u32 = 100;

const DESCRIPTION: &str = "Search file contents for a pattern. Returns matching lines with file paths and line numbers. Respects .gitignore. Output is truncated to 100 matches or 50KB (whichever is hit first). Long lines are truncated to 500 chars.";

/// `grep`'s arguments, pi's field names (`grep.ts`).
#[derive(Debug, Deserialize, JsonSchema)]
pub struct GrepArgs {
    /// Search pattern (regex or literal string)
    pub pattern: String,
    /// Directory or file to search (default: current directory)
    pub path: Option<String>,
    /// Filter files by glob pattern, e.g. '*.ts' or '**/*.spec.ts'
    pub glob: Option<String>,
    /// Case-insensitive search (default: false)
    #[serde(rename = "ignoreCase")]
    pub ignore_case: Option<bool>,
    /// Treat pattern as literal string instead of regex (default: false)
    pub literal: Option<bool>,
    /// Number of lines to show before and after each match (default: 0)
    pub context: Option<u32>,
    /// Maximum number of matches to return (default: 100)
    pub limit: Option<u32>,
}

/// Searches file contents for a pattern (`docs/reference/tools.md`,
/// "grep").
pub struct Grep {
    root: Root,
    schema: Value,
}

impl Grep {
    pub fn new(root: Root) -> Self {
        let schema = serde_json::to_value(schemars::schema_for!(GrepArgs))
            .expect("a generated schema is valid JSON");
        Self { root, schema }
    }
}

#[async_trait]
impl AgentTool for Grep {
    fn name(&self) -> &str {
        "grep"
    }

    fn description(&self) -> &str {
        DESCRIPTION
    }

    fn parameters(&self) -> &Value {
        &self.schema
    }

    async fn call(
        &self,
        args: Value,
        ctx: ToolCtx,
    ) -> anyhow::Result<ToolOutput> {
        let args: GrepArgs = serde_json::from_value(args)?;
        let root = self.root.clone();
        let cancel = ctx.cancel.clone();
        let text =
            tokio::task::spawn_blocking(move || run(&root, args, &cancel))
                .await??;
        Ok(ToolOutput::text(text))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineKind {
    Before,
    Match,
    After,
}

struct LineEntry {
    number: u64,
    kind: LineKind,
    text: String,
}

struct CollectSink<'a> {
    lines: &'a mut Vec<LineEntry>,
}

impl Sink for CollectSink<'_> {
    type Error = std::io::Error;

    fn matched(
        &mut self,
        _searcher: &Searcher,
        mat: &SinkMatch<'_>,
    ) -> Result<bool, Self::Error> {
        self.lines.push(LineEntry {
            number: mat.line_number().unwrap_or(0),
            kind: LineKind::Match,
            text: decode_line(mat.bytes()),
        });
        Ok(true)
    }

    fn context(
        &mut self,
        _searcher: &Searcher,
        context: &SinkContext<'_>,
    ) -> Result<bool, Self::Error> {
        let kind = match context.kind() {
            SinkContextKind::Before => LineKind::Before,
            SinkContextKind::After | SinkContextKind::Other => LineKind::After,
        };
        self.lines.push(LineEntry {
            number: context.line_number().unwrap_or(0),
            kind,
            text: decode_line(context.bytes()),
        });
        Ok(true)
    }
}

/// A match or context line's bytes, minus its line terminator. Safe as
/// UTF-8 because the caller only ever searches a whole file already
/// known to be valid UTF-8, and `\n` never appears inside a multi-byte
/// sequence.
fn decode_line(bytes: &[u8]) -> String {
    std::str::from_utf8(bytes)
        .expect("a line of a UTF-8 file is UTF-8")
        .trim_end_matches(['\n', '\r'])
        .to_owned()
}

/// The `glob` argument as a gitignore-style whitelist: a pattern with
/// no `/` matches a file's name at any depth, and matching honors
/// `**` the way a `.gitignore` line would (`ignore::overrides`, the
/// same mechanism ripgrep's own `--glob` uses).
fn build_glob(root: &Path, pattern: &str) -> anyhow::Result<Override> {
    Ok(OverrideBuilder::new(root).add(pattern)?.build()?)
}

/// The path a matched file is reported under: relative to the search
/// root with `/` separators when searching a directory, or just the
/// file's name when a single file was searched (pi, `grep.ts`
/// `formatPath`).
fn format_path(search_path: &Path, file: &Path, is_dir: bool) -> String {
    if is_dir {
        file.strip_prefix(search_path)
            .unwrap_or(file)
            .to_string_lossy()
            .replace('\\', "/")
    } else {
        file.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

fn files_under(
    search_path: &Path,
    overrides: Override,
    cancel: &CancellationToken,
) -> anyhow::Result<Vec<PathBuf>> {
    let mut walk = WalkBuilder::new(search_path);
    // `.gitignore` applies whether or not the tree sits inside an
    // actual git repository (`ignore`'s default requires one). The
    // user's personal global gitignore and `.git/info/exclude` are
    // not part of the spec and would otherwise leak in once
    // `require_git` no longer gates them.
    walk.hidden(false)
        .require_git(false)
        .git_global(false)
        .git_exclude(false)
        .overrides(overrides);
    let files = Mutex::new(Vec::new());
    walk.build_parallel().run(|| {
        let files = &files;
        Box::new(move |result| {
            if cancel.is_cancelled() {
                return WalkState::Quit;
            }
            if let Ok(entry) = result
                && entry.file_type().is_some_and(|t| t.is_file())
            {
                files.lock().expect("not poisoned").push(entry.into_path());
            }
            WalkState::Continue
        })
    });
    if cancel.is_cancelled() {
        return Err(anyhow!(ABORTED));
    }
    let mut files = files.into_inner().expect("not poisoned");
    files.sort();
    Ok(files)
}

/// The match and context lines of one file, or `None` when it cannot be
/// read or is not UTF-8.
fn search_file(
    searcher: &mut Searcher,
    matcher: &RegexMatcher,
    file: &Path,
) -> std::io::Result<Option<Vec<LineEntry>>> {
    let Ok(bytes) = std::fs::read(file) else {
        return Ok(None);
    };
    if std::str::from_utf8(&bytes).is_err() {
        return Ok(None);
    }
    let mut collected = Vec::new();
    searcher.search_slice(
        matcher,
        &bytes,
        &mut CollectSink {
            lines: &mut collected,
        },
    )?;
    Ok(Some(collected))
}

/// Searches `files` on every core, in their order, and returns each
/// file's lines (`None` for a file not searched).
///
/// Workers take files in order. Once the files finished so far, counted
/// from the first without a gap, hold more than `limit` matches, the
/// output is settled (it keeps `limit` matches and knows the limit was
/// reached), so no worker starts another file. The result is the same
/// as searching one file after another.
fn search_files(
    files: &[PathBuf],
    matcher: &RegexMatcher,
    context: usize,
    limit: usize,
    workers: usize,
    cancel: &CancellationToken,
) -> anyhow::Result<Vec<Option<Vec<LineEntry>>>> {
    struct Progress {
        results: Vec<Option<Vec<LineEntry>>>,
        done: Vec<bool>,
        /// Files `..settled` are all done.
        settled: usize,
        settled_matches: usize,
    }
    let progress = Mutex::new(Progress {
        results: (0..files.len()).map(|_| None).collect(),
        done: vec![false; files.len()],
        settled: 0,
        settled_matches: 0,
    });
    let next = AtomicUsize::new(0);
    let stop = AtomicBool::new(false);
    let workers = workers.min(files.len()).max(1);

    let work = || -> std::io::Result<()> {
        let mut searcher = SearcherBuilder::new()
            .line_number(true)
            .before_context(context)
            .after_context(context)
            .build();
        loop {
            if stop.load(Ordering::Relaxed) || cancel.is_cancelled() {
                return Ok(());
            }
            let index = next.fetch_add(1, Ordering::Relaxed);
            let Some(file) = files.get(index) else {
                return Ok(());
            };
            let lines = search_file(&mut searcher, matcher, file)?;
            let mut progress = progress.lock().expect("not poisoned");
            progress.results[index] = lines;
            progress.done[index] = true;
            while progress.settled < files.len()
                && progress.done[progress.settled]
            {
                let settled = progress.settled;
                let matches =
                    progress.results[settled].as_ref().map_or(0, |lines| {
                        lines
                            .iter()
                            .filter(|l| l.kind == LineKind::Match)
                            .count()
                    });
                progress.settled_matches += matches;
                progress.settled += 1;
            }
            if progress.settled_matches > limit {
                stop.store(true, Ordering::Relaxed);
            }
        }
    };
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers).map(|_| scope.spawn(work)).collect();
        handles.into_iter().try_for_each(|handle| {
            handle.join().expect("a search worker panicked")
        })
    })?;
    if cancel.is_cancelled() {
        return Err(anyhow!(ABORTED));
    }
    Ok(progress.into_inner().expect("not poisoned").results)
}

fn run(
    root: &Root,
    args: GrepArgs,
    cancel: &CancellationToken,
) -> anyhow::Result<String> {
    if cancel.is_cancelled() {
        return Err(anyhow!(ABORTED));
    }
    let search_path = root.resolve(args.path.as_deref().unwrap_or("."));
    if !search_path.exists() {
        return Err(anyhow!("Path not found: {}", search_path.display()));
    }
    let is_dir = search_path.is_dir();
    let limit = args.limit.unwrap_or(DEFAULT_LIMIT).max(1) as usize;
    let context = args.context.unwrap_or(0) as usize;

    let matcher = RegexMatcherBuilder::new()
        .case_insensitive(args.ignore_case.unwrap_or(false))
        .fixed_strings(args.literal.unwrap_or(false))
        .build(&args.pattern)?;

    let files = if is_dir {
        let overrides = match args.glob.as_deref() {
            Some(pattern) => build_glob(&search_path, pattern)?,
            None => Override::empty(),
        };
        files_under(&search_path, overrides, cancel)?
    } else {
        vec![search_path.clone()]
    };

    let mut output_lines: Vec<String> = Vec::new();
    let mut match_count = 0usize;
    let mut limit_reached = false;
    let mut lines_truncated = false;

    let workers = std::thread::available_parallelism().map_or(1, |n| n.get());
    let results =
        search_files(&files, &matcher, context, limit, workers, cancel)?;
    'files: for (file, collected) in files.iter().zip(results) {
        let Some(collected) = collected else { continue };
        if collected.is_empty() {
            continue;
        }

        let rel = format_path(&search_path, file, is_dir);
        let mut pending: Vec<LineEntry> = Vec::new();
        for entry in collected {
            match entry.kind {
                LineKind::Before => pending.push(entry),
                LineKind::Match => {
                    if match_count >= limit {
                        limit_reached = true;
                        break 'files;
                    }
                    match_count += 1;
                    for line in pending.drain(..) {
                        push_line(
                            &mut output_lines,
                            &rel,
                            &line,
                            &mut lines_truncated,
                        );
                    }
                    push_line(
                        &mut output_lines,
                        &rel,
                        &entry,
                        &mut lines_truncated,
                    );
                }
                LineKind::After => {
                    push_line(
                        &mut output_lines,
                        &rel,
                        &entry,
                        &mut lines_truncated,
                    );
                }
            }
        }
    }

    if match_count == 0 {
        return Ok("No matches found".to_owned());
    }

    let raw = output_lines.join("\n");
    let truncation = truncate_head(&raw, usize::MAX, MAX_BYTES);
    let truncated = truncation.truncated();
    let mut text = truncation.content;

    let mut notices = Vec::new();
    if limit_reached {
        notices.push(format!(
            "{limit} matches limit reached. Use limit={} for more, or refine pattern",
            limit * 2
        ));
    }
    if truncated {
        notices.push(format!("{} limit reached", format_size(MAX_BYTES)));
    }
    if lines_truncated {
        notices.push(format!(
            "Some lines truncated to {GREP_MAX_LINE} chars. Use read tool to see full lines"
        ));
    }
    if !notices.is_empty() {
        text.push_str(&format!("\n\n[{}]", notices.join(". ")));
    }
    Ok(text)
}

fn push_line(
    out: &mut Vec<String>,
    rel: &str,
    entry: &LineEntry,
    lines_truncated: &mut bool,
) {
    let (text, was_truncated) = truncate_line(&entry.text, GREP_MAX_LINE);
    if was_truncated {
        *lines_truncated = true;
    }
    let sep = if entry.kind == LineKind::Match {
        ':'
    } else {
        '-'
    };
    out.push(format!("{rel}{sep}{}{sep} {text}", entry.number));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// With one worker the files are searched strictly in order, so the
    /// search stops exactly when the output is settled: as soon as the
    /// files so far hold more than `limit` matches, and not before (a
    /// file past `limit` matches still decides the "limit reached"
    /// notice).
    #[test]
    fn search_stops_once_the_output_is_settled() {
        let dir = tempfile::tempdir().unwrap();
        let files: Vec<PathBuf> = (0..4)
            .map(|i| {
                let path = dir.path().join(format!("f{i}.txt"));
                std::fs::write(&path, "NEEDLE\n").unwrap();
                path
            })
            .collect();
        let matcher = RegexMatcherBuilder::new().build("NEEDLE").unwrap();
        let searched = |limit| {
            search_files(
                &files,
                &matcher,
                0,
                limit,
                1,
                &CancellationToken::new(),
            )
            .unwrap()
            .iter()
            .map(Option::is_some)
            .collect::<Vec<_>>()
        };
        assert_eq!(searched(1), [true, true, false, false]);
        assert_eq!(searched(2), [true, true, true, false]);
        assert_eq!(searched(4), [true, true, true, true]);
    }
}
