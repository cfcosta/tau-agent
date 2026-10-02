//! CodingTools artifact grants stored as plugin records.
//! This module stays available without host tools so clients can display grants.

use std::collections::HashMap;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArtifactMetadata {
    pub id: String,
    pub digest: String,
    pub size_bytes: u64,
}

impl<'de> Deserialize<'de> for ArtifactMetadata {
    fn deserialize<D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Fields {
            id: String,
            digest: String,
            size_bytes: u64,
        }
        let fields = Fields::deserialize(deserializer)?;
        let id =
            Uuid::parse_str(&fields.id).map_err(serde::de::Error::custom)?;
        if id.get_version_num() != 7
            || id.hyphenated().to_string() != fields.id
            || fields.digest.len() != 64
            || !fields.digest.bytes().all(|byte| {
                byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
            })
        {
            return Err(serde::de::Error::custom("invalid artifact metadata"));
        }
        Ok(Self {
            id: fields.id,
            digest: fields.digest,
            size_bytes: fields.size_bytes,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactGrant {
    pub artifact: ArtifactMetadata,
    pub owner_run_id: String,
    pub source: String,
    /// The observed source reached its drain boundary when published.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_complete: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ArtifactRecord {
    ArtifactGrant(ArtifactGrant),
}

/// Fold only this plugin's stored records, retaining immutable metadata.
/// Conflicting grants for an ID fail closed.
pub fn fold_grants(
    records: &[Value],
) -> Result<HashMap<String, ArtifactGrant>, String> {
    let mut grants = HashMap::new();
    for record in records {
        if record.get("kind").and_then(Value::as_str) != Some("artifact_grant")
        {
            continue;
        }
        let ArtifactRecord::ArtifactGrant(grant) =
            serde_json::from_value(record.clone())
                .map_err(|error| error.to_string())?;
        if grant.owner_run_id.is_empty() || grant.source.is_empty() {
            return Err("artifact grant has no owner or source".into());
        }
        let id = grant.artifact.id.clone();
        if let Some(previous) = grants.insert(id, grant.clone())
            && previous != grant
        {
            return Err("artifact ID has conflicting grants".into());
        }
    }
    Ok(grants)
}

#[cfg(feature = "host")]
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

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{ArtifactMetadata, ArtifactRecord, fold_grants};

    const ID: &str = "0199b283-f06a-722b-8c75-476700ee3488";
    const DIGEST: &str =
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn record() -> Value {
        json!({"kind":"artifact_grant","artifact":{
            "id": ID, "digest": DIGEST, "size_bytes": 5
        },"owner_run_id":"run-1","source":"trusted"})
    }

    #[test]
    fn stored_grants_ignore_other_record_kinds_and_reject_conflicts() {
        let value = record();
        let grants = fold_grants(&[
            json!({"kind":"old_record"}),
            value.clone(),
            value.clone(),
        ])
        .unwrap();
        assert_eq!(grants.len(), 1);
        assert_eq!(grants[ID].owner_run_id, "run-1");
        let parsed: ArtifactRecord =
            serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(parsed).unwrap(), value);

        let mut conflict = value.clone();
        conflict["artifact"]["size_bytes"] = json!(6);
        assert!(
            fold_grants(&[value, conflict])
                .unwrap_err()
                .contains("conflicting")
        );
    }

    #[test]
    fn stored_metadata_rejects_paths_digests_and_empty_provenance() {
        let mut traversal = record();
        traversal["artifact"]["id"] = json!("../../secrets");
        assert!(fold_grants(&[traversal]).is_err());
        let mut digest = record();
        digest["artifact"]["digest"] = json!("../invalid");
        assert!(fold_grants(&[digest]).is_err());
        let mut provenance = record();
        provenance["source"] = json!("");
        assert!(fold_grants(&[provenance]).is_err());

        // Older non-grant records remain readable without host features.
        assert!(
            fold_grants(&[json!({"kind":"legacy","path":"x"})])
                .unwrap()
                .is_empty()
        );
        let metadata: ArtifactMetadata =
            serde_json::from_value(record()["artifact"].clone()).unwrap();
        assert_eq!(metadata.id, ID);
    }
}
