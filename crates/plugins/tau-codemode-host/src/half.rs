//! tau-codemode's host half (ADR 0030).

use std::sync::Arc;

use serde_json::Value;
use tau_agent::{plugin::Plugin, tool::RunId};
use tau_codemode::{
    CodemodeUi,
    PLUGIN,
    modules,
    promotion,
    store::Record,
    ui::{Action, ActionReply, description, validate_selection},
};
use tau_jev::Jev;
use tau_ui_plugin::{HostCx, HostHalf, PluginInfo, RunCtx, Seam};

/// tau-codemode on the host.
#[derive(Debug, Clone, Copy, Default)]
pub struct CodemodeHost;

impl HostHalf for CodemodeHost {
    type Plugin = CodemodeUi;
    type Host = ();

    /// The `codemode` tool, asking the run's metered Jev when there is a
    /// key; without one, scripts run and `jev` is nil.
    async fn agent_plugins(
        &self,
        _host: &(),
        run: &RunCtx,
        _settings: &(),
    ) -> anyhow::Result<Vec<Box<dyn Plugin>>> {
        let jev = run.services.get::<Arc<dyn Jev>>().cloned();
        Ok(vec![Box::new(
            crate::Codemode::new(jev)
                .with_repository(run.repo.dir.join("codemode-modules")),
        )])
    }

    async fn catalog(
        &self,
        _host: &(),
        cx: &HostCx,
        _settings: &(),
    ) -> PluginInfo {
        let jev = cx.services.get::<Arc<dyn Jev>>().is_some();
        PluginInfo {
            description: description(jev),
            seams: vec![Seam::Start, Seam::Tools],
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
        let result: Result<(), String> = async {
            match serde_json::from_value::<Action>(action)
                .map_err(|_| "Invalid module action".to_owned())?
            {
                Action::Select { run, name, version } => {
                    let records = cx
                        .records(&run, PLUGIN)
                        .await
                        .map_err(|error| error.to_string())?;
                    let selection =
                        validate_selection(&records, &name, &version)?;
                    let body = serde_json::to_value(Record::Module(selection))
                        .map_err(|error| error.to_string())?;
                    cx.publish(&run, PLUGIN, &body)
                        .await
                        .map_err(|error| error.to_string())
                }
                Action::Promote {
                    run,
                    request_id,
                    decision,
                } => {
                    let cx = cx.clone();
                    tokio::task::spawn_blocking(move || {
                        approve_or_decline(&cx, &run, &request_id, decision)
                    })
                    .await
                    .map_err(|error| error.to_string())?
                    .map_err(|error| format!("Promotion: {error}"))
                }
            }
        }
        .await;
        Ok(result.err().map(|error| {
            serde_json::to_value(ActionReply { error })
                .expect("reply serializes")
        }))
    }
}

/// Decides a promotion request, under the repository's manifest lock.
/// Blocks: it holds a file lock across its reads of the store, so it
/// runs in `spawn_blocking` (ADR 0028).
#[allow(
    clippy::disallowed_methods,
    reason = "runs in spawn_blocking, holding a file lock across its store reads (ADR 0028)"
)]
fn approve_or_decline(
    cx: &HostCx,
    run: &RunId,
    request_id: &str,
    decision: promotion::Decision,
) -> Result<(), String> {
    use tau_ui_plugin::{HOST_RECORD, HostRecord};

    let host_records = cx
        .runtime
        .block_on(cx.store.plugin_entries_everywhere(HOST_RECORD))
        .map_err(|error| error.to_string())?;
    let own_hosts: Vec<_> = host_records
        .iter()
        .filter(|(owner, _)| owner.as_str() == run.0.as_ref())
        .collect();
    if own_hosts.len() != 1 {
        return Err("run has no unique persisted host record".into());
    }
    let host: HostRecord = serde_json::from_str(&own_hosts[0].1)
        .map_err(|_| "run host record is malformed".to_owned())?;
    let repo = cx
        .repo(&host.repo)
        .ok_or("run repository is not configured on this host")?;
    let repository = crate::repository_modules::RepositoryModules::new(
        repo.dir.join("codemode-modules"),
    );
    // Serialize the Store decision check and manifest mutation across hosts.
    let manifest_lock = repository.lock_manifest()?;
    let raw_records = cx
        .runtime
        .block_on(cx.store.records(&run.0, PLUGIN))
        .map_err(|error| error.to_string())?;
    let records: Vec<Value> = raw_records
        .iter()
        .map(|body| {
            serde_json::from_str(body)
                .map_err(|_| "malformed persisted codemode record".to_owned())
        })
        .collect::<Result<_, _>>()?;
    let (request, previous) = promotion::find_request(&records, request_id)?;
    if request.owner != run.0.as_ref() {
        return Err("promotion belongs to another run".into());
    }
    let matching_requests = cx
        .runtime
        .block_on(cx.store.plugin_entries_everywhere(PLUGIN))
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|(owner, body)| {
            serde_json::from_str::<Value>(&body)
                .map(|value| (owner, value))
                .map_err(|_| "malformed persisted codemode record".to_owned())
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|(_, body)| {
            body.get("kind").and_then(Value::as_str) == Some("promotion")
                && body.get("op").and_then(Value::as_str) == Some("requested")
                && body.get("id").and_then(Value::as_str) == Some(request_id)
        })
        .collect::<Vec<_>>();
    if matching_requests.len() != 1
        || matching_requests[0].0.as_str() != run.0.as_ref()
    {
        return Err(
            "promotion request ID is duplicated or owned by another run".into(),
        );
    }
    let request_index = records
        .iter()
        .position(|body| {
            body.get("kind").and_then(Value::as_str) == Some("promotion")
                && body.get("op").and_then(Value::as_str) == Some("requested")
                && body.get("id").and_then(Value::as_str) == Some(request_id)
        })
        .ok_or("promotion request is missing from persisted run records")?;
    let at_request = &records[..=request_index];
    let pin = modules::pin_for_run(at_request, &run.0)?
        .ok_or("repository pin is missing for this run")?;
    let scratch = modules::fold(at_request);
    request.verify(
        &scratch,
        &pin,
        &run.0,
        &repository.scope(),
        &repository.key(),
    )?;
    match previous {
        Some(promotion::Decision::Declined) => {
            return if decision == promotion::Decision::Declined {
                Ok(())
            } else {
                Err("promotion was already declined".into())
            };
        }
        Some(promotion::Decision::Approved)
            if decision == promotion::Decision::Declined =>
        {
            return Err("promotion was already approved".into());
        }
        Some(promotion::Decision::Approved) => {
            return if repository
                .has_receipt_with_lock(&request, &manifest_lock)?
            {
                Ok(())
            } else {
                Err("approved terminal record has no repository receipt".into())
            };
        }
        _ => {}
    }
    if decision == promotion::Decision::Declined
        && repository.has_receipt_with_lock(&request, &manifest_lock)?
    {
        return Err("promotion was already activated; retry approval to store its terminal record".into());
    }
    if decision == promotion::Decision::Approved {
        repository.approve_with_lock(&request, &manifest_lock)?;
    }
    if previous.is_none() {
        let terminal = promotion::Terminal {
            request_id: request.id,
            owner: request.owner,
            digest: request.digest,
            decision,
        };
        let body = serde_json::to_value(Record::Promotion(
            promotion::Record::Decided(terminal),
        ))
        .map_err(|error| error.to_string())?;
        cx.runtime
            .block_on(cx.publish(run, PLUGIN, &body))
            .map_err(|error| {
                format!(
                    "terminal record could not be stored; retry this decision: {error}"
                )
            })?;
    }
    Ok(())
}
