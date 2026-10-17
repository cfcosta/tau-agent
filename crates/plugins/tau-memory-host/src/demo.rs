//! The notes the demo shows, written where the real host half reads
//! them: what tau-agent, docbert and homelab.nix's runs kept.

use tau_ui_plugin::HostCx;

use crate::{
    half::now,
    note::{By, Link, LinkType, Note, NoteType, Source},
    store::Notes,
};

/// Writes the demo's notes into each listed repository's memory: the
/// ones the mockups show for tau-agent, a few for docbert and
/// homelab.nix.
pub fn seed(cx: &HostCx) -> anyhow::Result<()> {
    for repo in &cx.repos {
        let notes = match repo.name.as_str() {
            "tau-agent" => tau_agent(),
            "docbert" => docbert(),
            "homelab.nix" => homelab(),
            _ => continue,
        };
        let mut store = Notes::open(crate::half::repo_dir(repo))?;
        for note in notes {
            store.create(note)?;
        }
    }
    Ok(())
}

fn link(to: &str, why: &str) -> Link {
    Link {
        to: to.into(),
        kind: LinkType::Relates,
        why: Some(why.into()),
    }
}

/// A fact the agent kept `days` ago: its first paragraph is its
/// description, and it is about `paths`.
fn note(
    id: &str,
    title: &str,
    body: &[&str],
    links: Vec<Link>,
    paths: &[&str],
    days: u64,
) -> Note {
    let at = now().saturating_sub(days * 24 * 60 * 60 * 1000);
    let (description, rest) = body.split_first().unwrap_or((&"", &[]));
    Note {
        id: id.into(),
        title: title.into(),
        description: (*description).into(),
        kind: NoteType::Fact,
        tags: Vec::new(),
        created: at,
        updated: at,
        valid_from: at,
        valid_to: None,
        stale: None,
        source: Source {
            files: paths.iter().map(|path| (*path).to_owned()).collect(),
            ..Source::new(By::Agent)
        },
        links,
        body: rest.join("\n\n"),
    }
}

fn tau_agent() -> Vec<Note> {
    vec![
        note(
            "n-0417",
            "Rotation must drain lanes first",
            &[
                "A connection that is past its deadline can still carry lanes with a response in flight. Retiring it right away breaks their continuation: the next request would name a `previous_response_id` that the new socket has never seen.",
                "So the pool marks the connection as draining, sends no new lanes to it, and closes it when its last lane finishes.",
                "Jitter on the deadline only moves when draining starts. It does not replace it.",
            ],
            vec![
                link("n-0212", "how a lane knows what it continues"),
                link("n-0433", "jitter moves the deadline"),
            ],
            &[
                "crates/tau-ai/src/ws/proto/pool.rs",
                "crates/tau-ai/src/ws/proto/lane.rs",
            ],
            7,
        ),
        note(
            "n-0433",
            "Spread reconnects with jitter",
            &[
                "Connections opened together expire together unless the deadline moves. A jitter drawn at open time spreads the rotations.",
            ],
            vec![link("n-0417", "draining still applies")],
            &["crates/tau-ai/src/ws/proto/pool.rs"],
            2,
        ),
        note(
            "n-0212",
            "Continuation ids are call_id|item_id",
            &[
                "The Responses API needs both halves to resume a tool call. tau-ai joins them with a `|` in the tool call id.",
            ],
            vec![],
            &["crates/tau-ai/src/responses/input.rs"],
            9,
        ),
        note(
            "n-0301",
            "16 in flight per connection",
            &[
                "The pool enforces the limit per socket, not per client. Draining sockets still count.",
            ],
            vec![link("n-0417", "draining sockets still count")],
            &["crates/tau-ai/src/ws/proto/pool.rs"],
            3,
        ),
        note(
            "n-0388",
            "Retry policy honors server hints",
            &[
                "When the server sends `retry-after`, it wins over our backoff, capped at `max_delay`. The header can be seconds or an HTTP date.",
            ],
            vec![link("n-0212", "retries resume the same continuation")],
            &["crates/tau-ai/src/retry.rs"],
            4,
        ),
        note(
            "n-0390",
            "429 vs 503 in the Responses API",
            &[
                "429 means we sent too much; 503 means they are overloaded. Both are retried, and both may carry `retry-after`.",
            ],
            vec![link("n-0388", "both carry the hint")],
            &["crates/tau-ai/src/retry.rs"],
            2,
        ),
        note(
            "n-0205",
            "Tests use the fake OpenAI server",
            &[
                "`tau-testing` runs a fake Responses server over WebSocket. Tests never reach the network.",
            ],
            vec![],
            &["crates/tau-testing/src/fake_openai.rs"],
            11,
        ),
        note(
            "n-0350",
            "Rewrites force one full resend",
            &[
                "Any edit to the transcript breaks the delta chain once. The next request goes in full, and turns are deltas again after it.",
            ],
            vec![link("n-0417", "a resend can land on a draining socket")],
            &["crates/tau-agent/src/plugin.rs"],
            5,
        ),
    ]
}

fn docbert() -> Vec<Note> {
    vec![
        note(
            "d-0102",
            "Scanned pages have no text layer",
            &[
                "PDFs made from scans come in with empty pages. Run OCR on a page only when it has no text layer, so born-digital PDFs stay fast.",
            ],
            vec![],
            &["src/ingest/pdf.rs"],
            3,
        ),
        note(
            "d-0118",
            "Rerank only the top 50",
            &[
                "ColBERT reranking costs grow with the candidate list. BM25 recalls 200, and only the top 50 go to the reranker.",
            ],
            vec![link("d-0131", "BM25 recalls the candidates")],
            &["src/search/rerank.rs"],
            4,
        ),
        note(
            "d-0131",
            "The BM25 tokenizer keeps identifiers whole",
            &[
                "`snake_case` and `CamelCase` stay one token, and are also split into their parts, so both searches hit.",
            ],
            vec![],
            &["src/search/bm25.rs"],
            2,
        ),
    ]
}

fn homelab() -> Vec<Note> {
    vec![note(
        "h-0007",
        "Backups run from a systemd timer",
        &[
            "`restic` runs from `backup.timer` at 03:00, never from cron, so a missed run catches up on boot.",
        ],
        vec![],
        &["hosts/nas/backup.nix"],
        1,
    )]
}

/// What the demo's imports show: Claude Code and Codex on the demo's
/// computer, proposals for tau-agent, docbert and the person, the import
/// reading chats, and a banner on docbert. Synthetic: no one's real
/// notes.
pub fn imports() -> tau_memory::import::Imports {
    use tau_memory::import::{
        Banner,
        Flags,
        FolderFound,
        Imports,
        Material,
        Pending,
        Place,
        Progress,
        Tally,
        Tool,
        ToolFound,
    };
    let place = |name: &str| Place {
        folder: Some(format!("/home/demo/Code/cfcosta/{name}")),
        remote: Some(format!("git@github.com:cfcosta/{name}.git")),
    };
    let folder = |name: &str, saved: usize, chats: usize| FolderFound {
        place: place(name),
        label: format!("~/Code/cfcosta/{name}"),
        saved,
        chats,
        unread_saved: saved,
        unread_chats: chats,
    };
    let others = |n: usize, saved: usize| {
        (0..n).map(move |i| FolderFound {
            place: Place {
                folder: Some(format!("/home/demo/scratch/project-{i}")),
                remote: None,
            },
            label: format!("~/scratch/project-{i}"),
            saved,
            chats: 2,
            unread_saved: saved,
            unread_chats: 2,
        })
    };
    let pending = |id: &str,
                   kind: NoteType,
                   title: &str,
                   description: &str,
                   repo: Option<&str>,
                   origins: &[&str],
                   flags: Flags| Pending {
        id: id.into(),
        kind,
        title: title.into(),
        description: description.into(),
        repo: repo.map(str::to_owned),
        origins: origins.iter().map(|o| (*o).to_owned()).collect(),
        in_both: origins.iter().any(|o| o.starts_with("Claude Code"))
            && origins.iter().any(|o| o.starts_with("Codex")),
        keep: !flags.need_a_look(),
        flags,
    };
    let mut tally = Tally {
        left_out: 380,
        merged: 15,
        ..Tally::default()
    };
    tally
        .read
        .insert(Tally::key(Tool::ClaudeCode, Material::Saved), 501);
    tally
        .read
        .insert(Tally::key(Tool::Codex, Material::Saved), 17);
    Imports {
        found: vec![
            ToolFound {
                tool: Tool::ClaudeCode,
                saved: 501,
                chats: 180,
                about_you: 18,
                folders: [
                    folder("tau-agent", 41, 64),
                    folder("docbert", 9, 23),
                ]
                .into_iter()
                .chain(others(52, 8))
                .collect(),
            },
            ToolFound {
                tool: Tool::Codex,
                saved: 93,
                chats: 748,
                about_you: 0,
                folders: [
                    folder("tau-agent", 14, 120),
                    folder("docbert", 4, 30),
                ]
                .into_iter()
                .chain(others(31, 2))
                .collect(),
            },
        ],
        pending: vec![
            pending(
                "p-1",
                NoteType::Preference,
                "Answers lead with the result",
                "The outcome first, then the detail that backs it.",
                None,
                &["Claude Code · memory/result-first.md"],
                Flags::default(),
            ),
            pending(
                "p-2",
                NoteType::Preference,
                "Commits are small, and each one builds",
                "One change per commit; every commit passes the tests.",
                None,
                &[
                    "Claude Code · memory/small-commits.md",
                    "Codex · task summary, 2026-10-09",
                ],
                Flags::default(),
            ),
            pending(
                "p-3",
                NoteType::Procedure,
                "Run the integration suite with --features live",
                "Says the suite needs the staging database; tau has n-0441 saying it runs offline.",
                Some("tau-agent"),
                &["Codex · task summary, 2026-10-09"],
                Flags {
                    disagrees: Some("n-0441".into()),
                    ..Flags::default()
                },
            ),
            pending(
                "p-4",
                NoteType::Gotcha,
                "The fixture server needs a free port",
                "Bind port 0 and read it back; a fixed port collides in CI.",
                Some("tau-agent"),
                &["Claude Code · chat on 2026-10-04"],
                Flags::default(),
            ),
            pending(
                "p-5",
                NoteType::Fact,
                "[redacted] token for the staging API",
                "The chat pasted a secret; tau kept it out of the note.",
                Some("tau-agent"),
                &["Codex · chat on 2026-09-22"],
                Flags {
                    secret: true,
                    ..Flags::default()
                },
            ),
            pending(
                "p-6",
                NoteType::Fact,
                "Searches go through the BM25 index first",
                "Candidates come from BM25 before ColBERT reranks them.",
                Some("docbert"),
                &["Claude Code · memory/bm25-first.md"],
                Flags::default(),
            ),
            pending(
                "p-7",
                NoteType::Decision,
                "Embeddings are rebuilt when the model changes",
                "The cache is keyed by model, so a new model re-encodes.",
                Some("docbert"),
                &["Codex · task summary, 2026-10-02"],
                Flags::default(),
            ),
        ],
        held: 0,
        progress: Some(Progress {
            fraction: 0.36,
            line: "Reading Claude Code chats · 64 of 180".into(),
        }),
        tally,
        failed: vec![
            "~/.codex/sessions/2026/09/30/rollout-cut-short.jsonl".into(),
        ],
        banners: [(
            "docbert".to_owned(),
            Banner {
                folder: "~/Code/cfcosta/docbert".into(),
                places: vec![place("docbert")],
                saved: vec![(Tool::ClaudeCode, 9), (Tool::Codex, 4)],
                unread_chats: 23,
                unread_saved: 0,
                ready: 2,
            },
        )]
        .into(),
    }
}
