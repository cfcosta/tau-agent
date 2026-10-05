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
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tau_agent::{
    error::ToolError,
    tool::{AgentTool, ToolCtx, ToolOutput},
};
use tokio_util::sync::CancellationToken;

use crate::{
    ABORTED,
    SearchError,
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
    output_schema: Value,
}

impl Grep {
    pub fn new(root: Root) -> Self {
        let schema = serde_json::to_value(schemars::schema_for!(GrepArgs))
            .expect("a generated schema is valid JSON");
        let output_schema =
            serde_json::to_value(schemars::schema_for!(GrepOutput))
                .expect("a generated schema is valid JSON");
        Self {
            root,
            schema,
            output_schema,
        }
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

    fn output_schema(&self) -> Option<&Value> {
        Some(&self.output_schema)
    }

    async fn call(
        &self,
        args: Value,
        ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        let args: GrepArgs = serde_json::from_value(args)?;
        let root = self.root.clone();
        let cancel = ctx.cancel.clone();
        let result =
            tokio::task::spawn_blocking(move || run(&root, args, &cancel))
                .await??;
        Ok(ToolOutput {
            structured: Some(
                serde_json::to_value(result.structured)
                    .expect("grep output is valid JSON"),
            ),
            ..ToolOutput::text(result.text)
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
enum LineKind {
    Before,
    Match,
    After,
}

#[derive(Debug, Serialize, JsonSchema)]
struct GrepLine {
    path: String,
    line: u64,
    text: String,
    kind: LineKind,
    truncated: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
struct GrepOutput {
    root: String,
    lines: Vec<GrepLine>,
    match_count: usize,
    truncated: bool,
    complete: bool,
    limit_reached: bool,
    bytes_truncated: bool,
    lines_truncated: bool,
    skipped: usize,
}

struct GrepResult {
    text: String,
    structured: GrepOutput,
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
fn build_glob(root: &Path, pattern: &str) -> Result<Override, SearchError> {
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

struct WalkedFiles {
    files: Vec<PathBuf>,
    skipped: usize,
}

fn files_under(
    search_path: &Path,
    overrides: Override,
    cancel: &CancellationToken,
) -> Result<WalkedFiles, ToolError> {
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
    let skipped = AtomicUsize::new(0);
    walk.build_parallel().run(|| {
        let files = &files;
        let skipped = &skipped;
        Box::new(move |result| {
            if cancel.is_cancelled() {
                return WalkState::Quit;
            }
            match result {
                Ok(entry) => match entry.file_type() {
                    Some(file_type) if file_type.is_file() => {
                        files
                            .lock()
                            .expect("not poisoned")
                            .push(entry.into_path());
                    }
                    Some(_) => {}
                    None => {
                        skipped.fetch_add(1, Ordering::Relaxed);
                    }
                },
                Err(_) => {
                    skipped.fetch_add(1, Ordering::Relaxed);
                }
            }
            WalkState::Continue
        })
    });
    if cancel.is_cancelled() {
        return Err(ToolError::from(ABORTED));
    }
    let mut files = files.into_inner().expect("not poisoned");
    files.sort();
    Ok(WalkedFiles {
        files,
        skipped: skipped.load(Ordering::Relaxed),
    })
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

/// Searches `files` on every core, in their order, and records whether
/// each file was searched, skipped, or never started.
///
/// Workers take files in order. Once the files finished so far, counted
/// from the first without a gap, hold more than `limit` matches, the
/// output is settled (it keeps `limit` matches and knows the limit was
/// reached), so no worker starts another file. The result is the same
/// as searching one file after another.
enum FileSearch {
    /// This file was not started after the match limit settled.
    NotStarted,
    /// Reading failed or the file was not UTF-8.
    Skipped,
    /// The file was searched, including files with no matches.
    Searched(Vec<LineEntry>),
}

fn search_files(
    files: &[PathBuf],
    matcher: &RegexMatcher,
    context: usize,
    limit: usize,
    workers: usize,
    cancel: &CancellationToken,
) -> Result<Vec<FileSearch>, ToolError> {
    struct Progress {
        results: Vec<FileSearch>,
        /// Files `..settled` are all done.
        settled: usize,
        settled_matches: usize,
    }
    let progress = Mutex::new(Progress {
        results: (0..files.len()).map(|_| FileSearch::NotStarted).collect(),
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
            let result = match search_file(&mut searcher, matcher, file)? {
                Some(lines) => FileSearch::Searched(lines),
                None => FileSearch::Skipped,
            };
            let mut progress = progress.lock().expect("not poisoned");
            progress.results[index] = result;
            while progress.settled < files.len()
                && !matches!(
                    &progress.results[progress.settled],
                    FileSearch::NotStarted
                )
            {
                let settled = progress.settled;
                let matches = match &progress.results[settled] {
                    FileSearch::Searched(lines) => lines
                        .iter()
                        .filter(|l| l.kind == LineKind::Match)
                        .count(),
                    FileSearch::NotStarted | FileSearch::Skipped => 0,
                };
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
        return Err(ToolError::from(ABORTED));
    }
    Ok(progress.into_inner().expect("not poisoned").results)
}

/// The matcher for `args`' pattern, as its flags ask.
fn matcher(args: &GrepArgs) -> Result<RegexMatcher, SearchError> {
    Ok(RegexMatcherBuilder::new()
        .case_insensitive(args.ignore_case.unwrap_or(false))
        .fixed_strings(args.literal.unwrap_or(false))
        .build(&args.pattern)?)
}

fn run(
    root: &Root,
    args: GrepArgs,
    cancel: &CancellationToken,
) -> Result<GrepResult, ToolError> {
    if cancel.is_cancelled() {
        return Err(ToolError::from(ABORTED));
    }
    let search_path = root.resolve(args.path.as_deref().unwrap_or("."));
    if !search_path.exists() {
        return Err(ToolError::from(format!(
            "Path not found: {}",
            search_path.display()
        )));
    }
    let is_dir = search_path.is_dir();
    let limit = args.limit.unwrap_or(DEFAULT_LIMIT).max(1) as usize;
    let context = args.context.unwrap_or(0) as usize;

    let matcher = matcher(&args)?;

    let walked_files = if is_dir {
        let overrides = match args.glob.as_deref() {
            Some(pattern) => build_glob(&search_path, pattern)?,
            None => Override::empty(),
        };
        files_under(&search_path, overrides, cancel)?
    } else {
        WalkedFiles {
            files: vec![search_path.clone()],
            skipped: 0,
        }
    };
    let WalkedFiles {
        files,
        skipped: walker_skipped,
    } = walked_files;

    let mut output_lines: Vec<PresentedLine> = Vec::new();
    let mut match_count = 0usize;
    let mut limit_reached = false;
    let mut lines_truncated = false;

    let workers = std::thread::available_parallelism().map_or(1, |n| n.get());
    let results =
        search_files(&files, &matcher, context, limit, workers, cancel)?;
    let skipped = results.iter().fold(walker_skipped, |count, result| {
        count + usize::from(matches!(result, FileSearch::Skipped))
    });
    'files: for (file, collected) in files.iter().zip(results) {
        let FileSearch::Searched(collected) = collected else {
            continue;
        };
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
        return Ok(GrepResult {
            text: "No matches found".to_owned(),
            structured: GrepOutput {
                root: search_path.to_string_lossy().into_owned(),
                lines: Vec::new(),
                match_count,
                truncated: false,
                complete: skipped == 0,
                limit_reached,
                bytes_truncated: false,
                lines_truncated: false,
                skipped,
            },
        });
    }

    let raw = output_lines
        .iter()
        .map(|line| line.display.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let truncation = truncate_head(&raw, usize::MAX, MAX_BYTES);
    let bytes_truncated = truncation.truncated();
    let retained_bytes = truncation.content.len();
    let mut text = truncation.content;

    let mut notices = Vec::new();
    if limit_reached {
        notices.push(format!(
            "{limit} matches limit reached. Use limit={} for more, or refine pattern",
            limit * 2
        ));
    }
    if bytes_truncated {
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

    // Keep a structured line only when its whole original display record
    // fits in the presentation prefix. A byte cut can split a rendered path
    // (which may itself contain newlines), but it never creates a partial
    // structured record.
    let record_count = output_lines.len();
    let mut lines = Vec::new();
    let mut offset = 0;
    for (index, line) in output_lines.into_iter().enumerate() {
        let end = offset + line.display.len();
        if end <= retained_bytes {
            lines.push(line.record);
        }
        offset = end + usize::from(index + 1 < record_count);
    }
    let truncated = limit_reached || bytes_truncated || lines_truncated;
    let complete =
        skipped == 0 && !limit_reached && !bytes_truncated && !lines_truncated;
    Ok(GrepResult {
        text,
        structured: GrepOutput {
            root: search_path.to_string_lossy().into_owned(),
            lines,
            match_count,
            truncated,
            complete,
            limit_reached,
            bytes_truncated,
            lines_truncated,
            skipped,
        },
    })
}

struct PresentedLine {
    record: GrepLine,
    display: String,
}

fn push_line(
    out: &mut Vec<PresentedLine>,
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
    let display = format!("{rel}{sep}{}{sep} {text}", entry.number);
    out.push(PresentedLine {
        record: GrepLine {
            path: rel.to_owned(),
            line: entry.number,
            text,
            kind: entry.kind,
            truncated: was_truncated,
        },
        display,
    });
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
            .map(|result| matches!(result, FileSearch::Searched(_)))
            .collect::<Vec<_>>()
        };
        assert_eq!(searched(1), [true, true, false, false]);
        assert_eq!(searched(2), [true, true, true, false]);
        assert_eq!(searched(4), [true, true, true, true]);
    }
}
