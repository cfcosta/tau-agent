//! tau-tools' host half (ADR 0030).

use anyhow::Context as _;
use serde_json::Value;
use tau_agent::plugin::Plugin;
use tau_artifacts::{Artifact, Bytes, Encoding, Quotas};
use tau_tools::{
    ToolsUi,
    artifact_grant::fold_grants,
    ui::{Action, ActionReply, NAME},
};
use tau_ui_plugin::{HostCx, HostHalf, PluginInfo, RunCtx, Seam};

/// tau-tools on the host.
#[derive(Debug, Clone, Copy, Default)]
pub struct ToolsHost;

impl HostHalf for ToolsHost {
    type Plugin = ToolsUi;
    type Host = ();

    /// None here: the host builds the tools on the run's workspace,
    /// where they act.
    async fn agent_plugins(
        &self,
        _host: &(),
        _run: &RunCtx,
        _settings: &(),
    ) -> anyhow::Result<Vec<Box<dyn Plugin>>> {
        Ok(Vec::new())
    }

    async fn catalog(
        &self,
        _host: &(),
        _cx: &HostCx,
        _settings: &(),
    ) -> PluginInfo {
        PluginInfo {
            description: "read, bash, edit, write, grep, find and ls on the \
                          run's workspace"
                .into(),
            seams: vec![Seam::Tools],
            page: None,
            ..Default::default()
        }
    }

    async fn act(
        &self,
        _host: &(),
        action: Value,
        cx: &HostCx,
    ) -> anyhow::Result<Option<Value>> {
        let Action::Read {
            run,
            id,
            offset,
            encoding,
        } = serde_json::from_value(action)?;
        let read = async {
            let encoding = match encoding.as_str() {
                "utf8" => Encoding::Utf8,
                "base64" => Encoding::Base64,
                _ => anyhow::bail!("unsupported encoding"),
            };
            let starts = cx
                .store
                .plugin_entries(&run.0, tau_ui_plugin::HOST_RECORD)
                .await?;
            let (_, start) = starts.first().context("run has no repository")?;
            if starts.len() != 1 {
                anyhow::bail!("ambiguous repository");
            }
            let start: tau_ui_plugin::HostRecord = serde_json::from_str(start)?;
            let repo =
                cx.repo(&start.repo).context("repository unavailable")?;
            let records: Vec<Value> = cx
                .store
                .records(&run.0, NAME)
                .await?
                .into_iter()
                .map(|body| serde_json::from_str(&body))
                .collect::<Result<_, _>>()?;
            let grants = fold_grants(&records).map_err(anyhow::Error::msg)?;
            let grant = grants
                .get(&id)
                .context("artifact is not granted to this run")?;
            let artifact: Artifact =
                serde_json::from_value(serde_json::to_value(&grant.artifact)?)?;
            // Files: read off the async workers (ADR 0028).
            let dir = repo.dir.join("artifacts");
            let range = tokio::task::spawn_blocking(move || {
                Bytes::new(dir, Quotas::default())?.read_range(
                    &artifact,
                    offset,
                    1024,
                    encoding,
                    &tokio_util::sync::CancellationToken::new(),
                )
            })
            .await??;
            Ok::<_, anyhow::Error>(serde_json::to_value(range)?)
        }
        .await;
        Ok(Some(serde_json::to_value(match read {
            Ok(range) => ActionReply {
                id,
                range: Some(range),
                error: None,
            },
            Err(error) => ActionReply {
                id,
                range: None,
                error: Some(error.to_string()),
            },
        })?))
    }
}
