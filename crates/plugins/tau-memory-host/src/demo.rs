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
