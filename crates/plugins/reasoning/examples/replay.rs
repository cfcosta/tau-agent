//! Replays the latest stored runs through the effort policy with the
//! real Jev, and prints what it would pick next to what the runs went
//! out at. Needs `TYPESAFE_API_KEY` and the network:
//!
//! ```sh
//! cargo run -p tau-reasoning --example replay -- ~/.local/share/tau/runs.db 20
//! ```
//!
//! Each run replays on its own model. It reports decisions, not
//! savings (`tau_reasoning::replay`).

use std::{collections::BTreeMap, sync::Arc};

use tau_reasoning::{
    NAME,
    Reasoning,
    replay::{Entry, replay},
};
use tau_store::Store;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args.next().ok_or("usage: replay <runs.db> [runs]")?;
    let limit: u32 = args.next().map_or(Ok(20), |n| n.parse())?;
    let jev = Arc::new(tau_jev::TypeSafe::from_env()?);
    let reasoning = Reasoning::new(jev);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let store = Store::open(&path).await?;
        let (mut requests, mut asked, mut changed, mut cost) = (0, 0, 0, 0.0);
        // Stored effort → replayed effort, per request.
        let mut moves: BTreeMap<(String, String), usize> = BTreeMap::new();
        for run in store.recent_runs(limit).await? {
            let picker = reasoning.picker(&run.model);
            if !picker.scores() {
                continue;
            }
            let timeline: Vec<Entry> = store
                .timeline(&run.id)
                .await?
                .into_iter()
                .filter_map(|entry| match entry {
                    tau_store::Entry::Message { body, .. } => {
                        serde_json::from_str(&body).ok().map(Entry::Message)
                    }
                    tau_store::Entry::Plugin { plugin, body }
                        if plugin == NAME =>
                    {
                        serde_json::from_str(&body).ok().map(Entry::Record)
                    }
                    _ => None,
                })
                .collect();
            let decisions = replay(&picker, "", &timeline).await;
            println!(
                "run {} ({}, {} requests)",
                run.id,
                run.model,
                decisions.len()
            );
            let name = |effort: &Option<String>| {
                effort.clone().unwrap_or_else(|| "default".into())
            };
            for decision in &decisions {
                let how = match &decision.asked {
                    None => "lease held".to_owned(),
                    Some(Err(error)) => format!("failed: {error}"),
                    Some(Ok(choice)) => {
                        cost += choice.cost;
                        format!(
                            "{} {} at {:.2}, lease {}",
                            choice.kind,
                            choice.effort,
                            choice.confidence,
                            choice.lease.as_deref().unwrap_or("none"),
                        )
                    }
                };
                println!(
                    "  #{:<3} {:<9} stored {:<8} → {:<8} ({how})",
                    decision.request,
                    decision.step,
                    name(&decision.recorded),
                    name(&decision.runs_at),
                );
                requests += 1;
                asked += usize::from(decision.asked.is_some());
                changed += usize::from(decision.recorded != decision.runs_at);
                *moves
                    .entry((name(&decision.recorded), name(&decision.runs_at)))
                    .or_default() += 1;
            }
        }
        println!(
            "\n{requests} requests; Jev asked {asked} times (${cost:.4}); \
             {changed} would go out at another effort"
        );
        for ((from, to), count) in moves {
            println!("  {from:<8} → {to:<8} {count}");
        }
        Ok::<_, Box<dyn std::error::Error>>(())
    })
}
