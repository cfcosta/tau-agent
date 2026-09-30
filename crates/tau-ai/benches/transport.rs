//! Throughput of the WebSocket transport under many concurrent runs with
//! growing transcripts.
//!
//! Each run opens a session and plays `TURNS` turns. Every turn sends the
//! whole transcript, streams a response of `DELTAS` text deltas, and then
//! grows the transcript by that response and a user message of `PAYLOAD`
//! bytes, as a tool result would. The server lives in the same process,
//! on an in-memory duplex stream, and waits `THINK_MS` before each reply
//! (on its own task, so replies overlap as they do on a real connection).
//!
//! Everything the client does per turn is measured: building the
//! request, the delta rule, serialization, frame routing and parsing.
//!
//! ```text
//! cargo bench -p tau-ai --bench transport
//! RUNS=128 TURNS=60 PAYLOAD=16384 cargo bench -p tau-ai --bench transport
//! ```

use std::{
    io,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tau_ai::{
    client::OpenAi,
    event::{Accumulator, AssistantEvent},
    message::{Message, UserContent, UserMessage},
    responses::request::Settings,
    ws::{io::connection::Connector, proto::pool::Limits},
};
use tokio::{io::DuplexStream, sync::mpsc};
use tokio_tungstenite::tungstenite::{self, http};

fn env(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

#[derive(Clone, Copy)]
struct Config {
    runs: u64,
    turns: u64,
    payload: usize,
    deltas: u64,
    think: Duration,
}

/// What the server saw.
#[derive(Default)]
struct Counters {
    full: AtomicU64,
    delta: AtomicU64,
    bytes_in: AtomicU64,
}

struct DuplexConnector {
    config: Config,
    counters: Arc<Counters>,
}

impl Connector for DuplexConnector {
    type Stream = DuplexStream;

    async fn connect(&self) -> io::Result<DuplexStream> {
        let (client, server) = tokio::io::duplex(1 << 20);
        tokio::spawn(serve(server, self.config, self.counters.clone()));
        Ok(client)
    }

    fn request(&self) -> http::Request<()> {
        http::Request::builder()
            .uri("ws://bench/v1/responses")
            .header("Host", "bench")
            .header("Connection", "Upgrade")
            .header("Upgrade", "websocket")
            .header("Sec-WebSocket-Version", "13")
            .header(
                "Sec-WebSocket-Key",
                tungstenite::handshake::client::generate_key(),
            )
            .body(())
            .unwrap()
    }
}

async fn serve(stream: DuplexStream, config: Config, counters: Arc<Counters>) {
    let Ok(socket) = tokio_tungstenite::accept_async(stream).await else {
        return;
    };
    let (mut sink, mut source) = socket.split();
    let (frames, mut outgoing) = mpsc::unbounded_channel::<String>();
    tokio::spawn(async move {
        while let Some(frame) = outgoing.recv().await {
            if sink.send(tungstenite::Message::text(frame)).await.is_err() {
                return;
            }
        }
    });
    let mut next_id = 0u64;
    while let Some(Ok(message)) = source.next().await {
        let tungstenite::Message::Text(text) = message else {
            continue;
        };
        counters
            .bytes_in
            .fetch_add(text.len() as u64, Ordering::Relaxed);
        let body: Value = serde_json::from_str(&text).unwrap();
        if body.get("previous_response_id").is_some() {
            counters.delta.fetch_add(1, Ordering::Relaxed);
        } else {
            counters.full.fetch_add(1, Ordering::Relaxed);
        }
        next_id += 1;
        let frames = frames.clone();
        tokio::spawn(async move {
            if !config.think.is_zero() {
                tokio::time::sleep(config.think).await;
            }
            for frame in response(next_id, config.deltas) {
                if frames.send(frame.to_string()).is_err() {
                    return;
                }
            }
        });
    }
}

fn response(id: u64, deltas: u64) -> Vec<Value> {
    let response_id = format!("resp_{id}");
    let item_id = format!("msg_{id}");
    let mut text = String::new();
    let mut frames = vec![
        json!({"type": "response.created", "response": {"id": response_id}}),
        json!({
            "type": "response.output_item.added",
            "output_index": 0,
            "item": {"type": "message", "id": item_id, "role": "assistant", "content": []},
        }),
    ];
    for n in 0..deltas {
        let delta = format!("token{n} ");
        text.push_str(&delta);
        frames.push(json!({
            "type": "response.output_text.delta",
            "output_index": 0,
            "delta": delta,
        }));
    }
    frames.push(json!({
        "type": "response.output_item.done",
        "output_index": 0,
        "item": {
            "type": "message",
            "id": item_id,
            "role": "assistant",
            "content": [{"type": "output_text", "text": text}],
        },
    }));
    frames.push(json!({
        "type": "response.completed",
        "response": {
            "id": response_id,
            "status": "completed",
            "usage": {"input_tokens": 100, "output_tokens": deltas, "total_tokens": 100 + deltas},
        },
    }));
    frames
}

/// One run. Returns each turn's latency, from the request to `Done`.
async fn run(client: OpenAi, config: Config) -> Vec<Duration> {
    let settings = Settings {
        model: "gpt-bench".into(),
        instructions: Some("You are a benchmark.".into()),
        ..Settings::default()
    };
    let mut session = client.session(settings).await.unwrap();
    let payload = "x".repeat(config.payload);
    let mut transcript = vec![user("start".into())];
    let mut latencies = Vec::with_capacity(config.turns as usize);
    for _ in 0..config.turns {
        let started = Instant::now();
        let mut response = session.respond(&transcript, 0);
        let mut accumulator = Accumulator::new();
        while let Some(event) = response.next().await {
            let done = matches!(event, AssistantEvent::Done { .. });
            accumulator.push(event).unwrap();
            if done {
                break;
            }
        }
        latencies.push(started.elapsed());
        let message = accumulator.finish().unwrap();
        transcript.push(Message::Assistant(message));
        transcript.push(user(payload.clone()));
    }
    latencies
}

fn user(text: String) -> Message {
    Message::User(UserMessage {
        content: UserContent::Text(text),
        timestamp: 0,
    })
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    let index = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[index]
}

fn main() {
    let config = Config {
        runs: env("RUNS", 64),
        turns: env("TURNS", 40),
        payload: env("PAYLOAD", 8192) as usize,
        deltas: env("DELTAS", 200),
        think: Duration::from_millis(env("THINK_MS", 5)),
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let counters = Arc::new(Counters::default());
        let client = OpenAi::with_connector(
            DuplexConnector {
                config,
                counters: counters.clone(),
            },
            Limits::default(),
        );
        let started = Instant::now();
        let runs: Vec<_> = (0..config.runs)
            .map(|_| tokio::spawn(run(client.clone(), config)))
            .collect();
        let mut latencies = Vec::new();
        for run in runs {
            latencies.extend(run.await.unwrap());
        }
        let wall = started.elapsed();
        latencies.sort();
        let turns = latencies.len() as f64;
        let deltas = turns * config.deltas as f64;
        println!(
            "runs={} turns={} payload={}B deltas={} think={:?}",
            config.runs,
            config.turns,
            config.payload,
            config.deltas,
            config.think
        );
        println!(
            "wall {:.3}s  {:.0} turns/s  {:.0} deltas/s",
            wall.as_secs_f64(),
            turns / wall.as_secs_f64(),
            deltas / wall.as_secs_f64()
        );
        println!(
            "turn latency p50 {:?}  p90 {:?}  p99 {:?}  max {:?}",
            percentile(&latencies, 0.5),
            percentile(&latencies, 0.9),
            percentile(&latencies, 0.99),
            latencies[latencies.len() - 1]
        );
        println!(
            "requests full {} delta {}  bytes sent {:.1} MiB",
            counters.full.load(Ordering::Relaxed),
            counters.delta.load(Ordering::Relaxed),
            counters.bytes_in.load(Ordering::Relaxed) as f64
                / (1024.0 * 1024.0)
        );
    });
}
