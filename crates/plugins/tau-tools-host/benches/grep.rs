//! Speed of the `grep` tool on a generated tree.
//!
//! The tree has `FILES` files of about `FILE_KB` KiB of text each, in 64
//! directories, plus an ignored directory the walk must skip. Each case
//! runs `ITERS` times after a warm-up; the median is reported.
//!
//! ```text
//! cargo bench -p tau-tools-host --bench grep
//! FILES=20000 FILE_KB=16 cargo bench -p tau-tools-host --bench grep
//! ```

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};

use serde_json::{Value, json};
use tau_agent::tool::{AgentTool, ToolCtx};
use tau_tools_host::{grep::Grep, path::Root};

const WORDS: &[&str] = &[
    "the",
    "agent",
    "loop",
    "calls",
    "a",
    "tool",
    "with",
    "arguments",
    "and",
    "streams",
    "its",
    "result",
    "back",
    "into",
    "transcript",
    "while",
    "compaction",
    "keeps",
    "context",
    "small",
    "fn",
    "let",
    "match",
    "impl",
];

fn env(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// A deterministic tree: every 997th file holds `needle_rare` once.
fn generate(root: &Path, files: usize, file_kb: usize) {
    let mut state = 0x2545_f491_4f6c_dd1du64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for n in 0..files {
        let dir = root.join(format!("dir{:02}", n % 64));
        fs::create_dir_all(&dir).unwrap();
        let mut text = String::with_capacity(file_kb * 1024 + 128);
        let mut line = 0;
        while text.len() < file_kb * 1024 {
            line += 1;
            for _ in 0..(6 + next() % 10) {
                text.push_str(WORDS[(next() % WORDS.len() as u64) as usize]);
                text.push(' ');
            }
            if n % 997 == 0 && line == 50 {
                text.push_str("needle_rare");
            }
            text.push('\n');
        }
        fs::write(dir.join(format!("file{n}.rs")), text).unwrap();
    }
    let ignored = root.join("target");
    fs::create_dir_all(&ignored).unwrap();
    for n in 0..files / 10 {
        fs::write(ignored.join(format!("junk{n}.rs")), "needle_rare\n")
            .unwrap();
    }
    fs::write(root.join(".gitignore"), "target/\n").unwrap();
}

fn main() {
    let files = env("FILES", 4000);
    let file_kb = env("FILE_KB", 32);
    let iters = env("ITERS", 7);
    let dir = tempfile::tempdir().unwrap();
    generate(dir.path(), files, file_kb);
    let tool = Grep::new(Root::new(dir.path()));
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();

    println!(
        "files={files} file_kb={file_kb} (~{} MiB) iters={iters}",
        files * file_kb / 1024
    );
    let cases: [(&str, Value); 5] = [
        (
            "rare literal",
            json!({"pattern": "needle_rare", "literal": true}),
        ),
        ("no match regex", json!({"pattern": "zq[0-9]{4}x"})),
        (
            "ignore case",
            json!({"pattern": "NEEDLE_RARE", "ignoreCase": true}),
        ),
        ("common, limit 100", json!({"pattern": "compaction keeps"})),
        (
            "common, limit 5000",
            json!({"pattern": "compaction keeps", "limit": 5000}),
        ),
    ];
    for (name, args) in cases {
        let mut times: Vec<Duration> = (0..=iters)
            .map(|_| {
                let started = Instant::now();
                runtime
                    .block_on(tool.call(args.clone(), ToolCtx::detached()))
                    .unwrap();
                started.elapsed()
            })
            .skip(1)
            .collect();
        times.sort();
        println!("{name:<20} median {:?}", times[times.len() / 2]);
    }
}
