//! An event's data too large for its message (ADR 0037): kept whole as
//! a codemode artifact of the agent's repository, granted to its main
//! chat, and named in the message in its place. Its runs read it with
//! `artifact_read`, a page at a time, from a script.

use std::{io::Cursor, path::Path};

use serde_json::{Value, json};
use tau_agents::event::INLINE_DATA;
use tau_artifacts::{Bytes, Quotas};
use tau_tools_host::artifact_grant::{
    ArtifactGrant,
    ArtifactMetadata,
    ArtifactRecord,
};
use tokio_util::sync::CancellationToken;

/// The directory of an agent's artifacts, beside its workspaces: where
/// its runs' artifacts are too.
pub const DIR: &str = "artifacts";

/// How much of the data the message keeps, ahead of the reference, so
/// the agent sees what it is before it reads it.
pub const HEAD: usize = 1024;

/// What the message holds in place of `data`, once it is kept as
/// `artifact`: the reference, its size, and the data's first
/// [`HEAD`] bytes.
pub fn reference(artifact: &ArtifactMetadata, data: &str) -> Value {
    let mut cut = HEAD.min(data.len());
    while !data.is_char_boundary(cut) {
        cut -= 1;
    }
    json!({
        "artifact": artifact.id,
        "size_bytes": artifact.size_bytes,
        "read_with": "artifact_read",
        "head": &data[..cut],
    })
}

/// Whether `data` is too large for its event's message.
pub fn too_large(data: &Value) -> bool {
    data.to_string().len() > INLINE_DATA
}

/// Keeps `data` as an artifact in the agent's `repo`, granted to the run
/// `run` as tau-tools' record (what `artifact_read` reads grants from),
/// written by `grant`. What its message holds instead.
pub async fn keep<F, Fut>(
    repo: &Path,
    run: &str,
    data: &Value,
    grant: F,
) -> Result<Value, String>
where
    F: FnOnce(Value) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    let text = data.to_string();
    let bytes = text.clone().into_bytes();
    let dir = repo.join(DIR);
    let published = tokio::task::spawn_blocking(move || {
        let store = Bytes::new(&dir, Quotas::default())?;
        store.publish_leased_reader(
            Cursor::new(bytes),
            &CancellationToken::new(),
        )
    })
    .await
    .map_err(|error| format!("cannot keep the data: {error}"))?
    .map_err(|error| format!("cannot keep the data: {error}"))?;
    let (artifact, _lease) = published;
    let metadata = ArtifactMetadata {
        id: artifact.id().to_owned(),
        digest: artifact.digest().to_owned(),
        size_bytes: artifact.size_bytes(),
    };
    let record = ArtifactRecord::ArtifactGrant(ArtifactGrant {
        artifact: metadata.clone(),
        owner_run_id: run.to_owned(),
        source: "tau-agents event".to_owned(),
        source_complete: Some(true),
    });
    let record = serde_json::to_value(&record)
        .map_err(|error| format!("cannot grant the data: {error}"))?;
    grant(record)
        .await
        .map_err(|error| format!("cannot grant the data: {error:#}"))?;
    Ok(reference(&metadata, &text))
}
