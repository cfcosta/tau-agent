//! tau-constitution's host half (ADR 0030): each repository's rules
//! as runs check them, the plugin's own database, and what its page
//! asks.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use serde_json::Value;
use tau_agent::plugin::Plugin;
use tau_constitution::{
    ConstitutionUi,
    ui::{Act, Data, RuleInfo, Rules, Stats, TrialResult, starting_status},
};
use tau_jev::Jev;
use tau_ui_plugin::{
    HostCx,
    HostHalf,
    Link,
    PluginHost,
    PluginInfo,
    RepoCtx,
    RunCtx,
    Seam,
    needs_jev,
};

use crate::{
    Constitution,
    ConstitutionPlugin,
    Live,
    NAME,
    OnError,
    Record,
    db::{self, Db},
    rules::Target,
};

/// The plugin on the host: each repository's rules as runs check them,
/// and where it keeps what a person looked at.
pub struct Host {
    /// The plugin's own database, opened when first needed, once: two
    /// opening it at once would both migrate it.
    db: tokio::sync::OnceCell<Db>,
    path: PathBuf,
    /// By [`rules_key`]: an edit replaces them here, and every run's
    /// next check reads the new ones.
    constitutions: Mutex<HashMap<String, Live>>,
}

/// What a repository's rules are stored under: its checkout.
fn rules_key(repo: &RepoCtx) -> String {
    std::fs::canonicalize(&repo.checkout)
        .unwrap_or_else(|_| repo.checkout.clone())
        .display()
        .to_string()
}

impl Host {
    /// The plugin's database, opened the first time.
    async fn db(&self) -> anyhow::Result<&Db> {
        Ok(self.db.get_or_try_init(|| Db::open(&self.path)).await?)
    }

    /// Repository `repo`'s rules as runs check them, read from the
    /// database the first time.
    async fn constitution(&self, repo: &RepoCtx) -> anyhow::Result<Live> {
        let key = rules_key(repo);
        if let Some(live) =
            self.constitutions.lock().expect("not poisoned").get(&key)
        {
            return Ok(live.clone());
        }
        let loaded = db::load(self.db().await?, &key).await?;
        // Another call may have loaded them meanwhile: the first stays.
        let mut open = self.constitutions.lock().expect("not poisoned");
        Ok(open.entry(key).or_insert_with(|| Live::new(loaded)).clone())
    }

    /// The repository named `name`.
    fn repo<'a>(cx: &'a HostCx, name: &str) -> anyhow::Result<&'a RepoCtx> {
        cx.repos
            .iter()
            .find(|repo| repo.name == name)
            .ok_or_else(|| anyhow::anyhow!("No repository {name}"))
    }

    /// Changes `repo`'s constitution with `edit`, which checks what it
    /// adds, and saves it. Runs going on check with the new rules from
    /// their next tool call.
    async fn edit(
        &self,
        cx: &HostCx,
        repo: &str,
        edit: impl FnOnce(&mut Constitution) -> Result<(), crate::RuleError>,
    ) -> anyhow::Result<()> {
        let repo = Self::repo(cx, repo)?;
        let live = self.constitution(repo).await?;
        let mut constitution = (*live.get()).clone();
        edit(&mut constitution)?;
        db::save(self.db().await?, &rules_key(repo), &constitution).await?;
        live.set(constitution);
        Ok(())
    }

    /// Replaces `repo`'s stored constitution with an empty one: the way
    /// out when the stored one cannot be read, which no edit can fix.
    async fn reset(&self, cx: &HostCx, repo: &str) -> anyhow::Result<()> {
        let repo = Self::repo(cx, repo)?;
        let key = rules_key(repo);
        let fresh = Constitution::default();
        db::save(self.db().await?, &key, &fresh).await?;
        let mut open = self.constitutions.lock().expect("not poisoned");
        match open.get(&key) {
            Some(live) => live.set(fresh),
            None => {
                open.insert(key, Live::new(fresh));
            }
        }
        Ok(())
    }

    /// What a person reviewed; none when the database cannot be read.
    async fn reviewed(&self) -> Vec<(String, String)> {
        let reviewed = match self.db().await {
            Ok(db) => db.reviewed().await.map_err(anyhow::Error::from),
            Err(error) => Err(error),
        };
        reviewed.unwrap_or_else(|error| {
            eprintln!("{NAME}: could not read what was reviewed: {error:#}");
            Vec::new()
        })
    }

    async fn set_reviewed(
        &self,
        run: String,
        key: String,
    ) -> anyhow::Result<()> {
        Ok(self.db().await?.mark_reviewed(&run, &key).await?)
    }

    /// What the checks did in each stored run of `repo`, from the
    /// plugin's records, counted as a run's view counts them.
    async fn history(
        &self,
        repo: &RepoCtx,
        cx: &HostCx,
    ) -> Vec<(String, Stats)> {
        let (Ok(records), Ok(ours)) =
            (cx.records_everywhere(NAME).await, cx.runs_in(repo).await)
        else {
            return Vec::new();
        };
        let mut history: Vec<(String, Stats)> = Vec::new();
        for (run, body) in records {
            if !ours.contains(&run) {
                continue;
            }
            let run = run.0.to_string();
            // Records come by run, so a run's are together.
            if history.last().is_none_or(|(last, _)| *last != run) {
                history.push((run, Stats::default()));
            }
            if let Some((_, stats)) = history.last_mut()
                && let Some(record) = Record::parse(&body)
            {
                stats.add(&record, None);
            }
        }
        history
    }
}

/// tau-constitution on the host.
#[derive(Debug, Clone, Copy, Default)]
pub struct ConstitutionHost;

impl HostHalf for ConstitutionHost {
    type Plugin = ConstitutionUi;
    type Host = Host;

    /// The repository's rules, checked with Jev when there is a key, in
    /// a run and its sub-agents alike. Rules that cannot be read fail
    /// the run: they are never skipped.
    async fn agent_plugins(
        &self,
        host: &Host,
        run: &RunCtx,
        _settings: &(),
    ) -> anyhow::Result<Vec<Box<dyn Plugin>>> {
        let Some(jev) = run.services.get::<Arc<dyn Jev>>() else {
            return Ok(Vec::new());
        };
        let rules = host.constitution(&run.repo).await?;
        Ok(vec![Box::new(ConstitutionPlugin::live(jev.clone(), rules))])
    }

    async fn starting(
        &self,
        host: &Host,
        run: &RunCtx,
        _settings: &(),
    ) -> Vec<Record> {
        let jev = run.services.get::<Arc<dyn Jev>>().is_some();
        let rules = host
            .constitution(&run.repo)
            .await
            .ok()
            .map(|live| live.get().rules.len());
        vec![Record::Starting {
            status: Some(starting_status(jev, rules)),
        }]
    }

    async fn catalog(
        &self,
        host: &Host,
        cx: &HostCx,
        _settings: &(),
    ) -> PluginInfo {
        let jev = cx.services.get::<Arc<dyn Jev>>().is_some();
        let mut rules = 0;
        for repo in &cx.repos {
            if let Ok(live) = host.constitution(repo).await {
                rules += live.get().rules.len();
            }
        }
        PluginInfo {
            group: tau_ui_plugin::Group::Rules,
            description: if jev {
                format!(
                    "{rules} rules across your repositories, checked with Jev"
                )
            } else {
                needs_jev(false, "Checks calls against each repository's rules")
            },
            seams: vec![Seam::BeforeTool, Seam::BeforeStop],
            page: Some(Link::page("rules").param("repo", "")),
            ..Default::default()
        }
    }

    async fn data(&self, host: &Host, _cx: &HostCx) -> Data {
        Data {
            reviewed: host.reviewed().await,
        }
    }

    async fn repo_data(
        &self,
        host: &Host,
        repo: &RepoCtx,
        cx: &HostCx,
    ) -> Rules {
        let history = host.history(repo, cx).await;
        match host.constitution(repo).await {
            Ok(live) => {
                let loaded = live.get();
                Rules {
                    rules: loaded
                        .rules
                        .iter()
                        .map(|rule| RuleInfo {
                            id: rule.id.clone(),
                            text: rule.text.clone(),
                            applies_to: rule
                                .on
                                .iter()
                                .map(Target::label)
                                .collect(),
                            review: rule.review,
                            block: rule.block,
                        })
                        .collect(),
                    max_holds: loaded.max_holds,
                    blocks_unchecked: loaded.on_error == OnError::Block,
                    error: None,
                    history,
                }
            }
            Err(error) => Rules {
                error: Some(format!("{error:#}")),
                history,
                ..Rules::default()
            },
        }
    }

    async fn act(
        &self,
        host: &Host,
        action: Value,
        cx: &HostCx,
    ) -> anyhow::Result<Option<Value>> {
        let act: Act = serde_json::from_value(action)?;
        let (done, failed) = match act {
            Act::Try {
                text,
                on,
                review,
                block,
                calls,
                answers,
            } => {
                let result =
                    try_rule(cx, &text, &on, review, block, &calls, &answers)
                        .await;
                return Ok(Some(serde_json::to_value(result)?));
            }
            Act::Reviewed { run, key } => (
                host.set_reviewed(run, key).await,
                "Could not save the review",
            ),
            Act::Add {
                repo,
                text,
                on,
                review,
                block,
            } => (
                host.edit(cx, &repo, |rules| {
                    rules.add(&text, &on, review, block).map(drop)
                })
                .await,
                "Could not add the rule",
            ),
            Act::Update {
                repo,
                id,
                text,
                on,
                review,
                block,
            } => (
                host.edit(cx, &repo, |rules| {
                    rules.replace(&id, &text, &on, review, block)
                })
                .await,
                "Could not save the rule",
            ),
            Act::Remove { repo, id } => (
                host.edit(cx, &repo, |rules| {
                    rules.remove(&id);
                    Ok(())
                })
                .await,
                "Could not remove the rule",
            ),
            Act::Settings {
                repo,
                blocks_unchecked,
                max_holds,
            } => (
                host.edit(cx, &repo, |rules| {
                    rules.on_error = if blocks_unchecked {
                        OnError::Block
                    } else {
                        OnError::Allow
                    };
                    rules.max_holds = max_holds;
                    Ok(())
                })
                .await,
                "Could not save the constitution",
            ),
            Act::Reset { repo } => {
                (host.reset(cx, &repo).await, "Could not remove the rules")
            }
        };
        if let Err(error) = done {
            cx.alert(failed, format!("{error:#}"));
        }
        // What the page shows comes from the host again, saved or not.
        cx.refresh();
        Ok(None)
    }
}

impl PluginHost for Host {
    async fn new(cx: &HostCx) -> anyhow::Result<Self> {
        Ok(Host {
            db: tokio::sync::OnceCell::new(),
            path: cx.plugin_dir(NAME).join("constitution.db"),
            constitutions: Mutex::default(),
        })
    }
}

/// Asks Jev what a rule being written makes of past `calls` and
/// `answers`, as a check would ask. Blocks.
async fn try_rule(
    cx: &HostCx,
    text: &str,
    on: &[String],
    review: f64,
    block: f64,
    calls: &[(String, Value)],
    answers: &[String],
) -> TrialResult {
    let on = on
        .iter()
        .map(|place| Target::parse(place))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    let rule = crate::Rule {
        id: "new".into(),
        text: text.to_owned(),
        on,
        review,
        block,
    };
    let jev = cx.services.get::<Arc<dyn Jev>>().ok_or_else(|| {
        "Trying a rule asks Jev: add a TypeSafe key on the Models screen."
            .to_owned()
    })?;
    crate::try_rule(&**jev, &rule, calls, answers).await
}
