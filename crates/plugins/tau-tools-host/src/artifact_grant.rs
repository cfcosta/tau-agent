//! Artifact grants: the shapes and their fold are the interface half's
//! (`tau_tools::artifact_grant`); publishing an artifact is the host's.

pub use tau_tools::artifact_grant::*;

pub async fn publish_artifact<R: std::io::Read + Send + 'static>(
    bytes: &tau_artifacts::Bytes,
    reader: R,
    source: &str,
    ctx: &tau_agent::tool::ToolCtx,
) -> Result<ArtifactGrant, tau_agent::error::ToolError> {
    use tau_agent::error::ToolError;

    struct CancelOnDrop(tokio_util::sync::CancellationToken);
    impl Drop for CancelOnDrop {
        fn drop(&mut self) {
            self.0.cancel();
        }
    }

    let Some(plugin) = ctx.plugin().cloned() else {
        return Err(ToolError::from(
            "artifact grant requires a tool plugin context",
        ));
    };
    if plugin.plugin() != crate::ui::NAME || source.is_empty() {
        return Err(ToolError::from(
            "artifact grant requires CodingTools and a source",
        ));
    }
    let storage = bytes.clone();
    // Dropping this future (for example on a script timeout) need not
    // cancel the parent tool context. The child stops the blocking reader.
    let cancel = ctx.cancel.child_token();
    let _cancel_on_drop = CancelOnDrop(cancel.clone());
    let work = tokio::task::spawn_blocking(move || {
        storage.publish_leased_reader(reader, &cancel)
    });
    let (artifact, _publication_lease) = tokio::select! {
        biased;
        _ = ctx.cancel.cancelled() => return Err(ToolError::from(crate::ABORTED)),
        result = work => result?.map_err(ToolError::other)?,
    };
    if ctx.cancel.is_cancelled() {
        return Err(ToolError::from(crate::ABORTED));
    }
    let grant = ArtifactGrant {
        artifact: ArtifactMetadata {
            id: artifact.id().to_owned(),
            digest: artifact.digest().to_owned(),
            size_bytes: artifact.size_bytes(),
        },
        owner_run_id: ctx.run.to_string(),
        source: source.to_owned(),
        source_complete: Some(true),
    };
    plugin
        .record(&ArtifactRecord::ArtifactGrant(grant.clone()))
        .await
        .map_err(ToolError::other)?;
    Ok(grant)
}
