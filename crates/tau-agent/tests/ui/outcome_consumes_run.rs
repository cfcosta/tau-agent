// `outcome` consumes the run: a finished run cannot be steered.
use tau_agent::agent::Run;

async fn finish(run: Run) {
    let _ = run.outcome().await;
    run.steer("too late");
}

fn main() {
    let _ = finish;
}
