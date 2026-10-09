//! The interface asking the host for something, and the host answering
//! it on its runtime (ADR 0028): nothing here waits on the interface's
//! thread.

use gpui::Context;

use super::*;

/// Runs `job`'s future on the host's runtime, then hands what it gave
/// to the workspace: `done` with its value, or `failed` with what went
/// wrong (ADR 0028). GPUI awaits the task without blocking a thread,
/// and nothing of it runs on the interface's thread.
fn on_host<T, F>(
    host: &Arc<Host>,
    workspace: &Entity<Workspace>,
    job: impl FnOnce(Arc<Host>) -> F + Send + 'static,
    done: impl FnOnce(&mut Workspace, T, &mut Context<Workspace>) + 'static,
    failed: impl FnOnce(&mut Workspace, String, &mut Context<Workspace>) + 'static,
    cx: &mut App,
) where
    T: Send + 'static,
    F: Future<Output = anyhow::Result<T>> + Send + 'static,
{
    let counted = host.job();
    let job = host.spawn(job);
    let workspace = workspace.downgrade();
    cx.spawn(async move |cx| {
        let result = match job.await {
            Ok(result) => result.map_err(|error| format!("{error:#}")),
            Err(error) => Err(error.to_string()),
        };
        let _ = workspace.update(cx, |ws, cx| match result {
            Ok(value) => done(ws, value, cx),
            Err(error) => failed(ws, error, cx),
        });
        drop(counted);
    })
    .detach();
}

/// A failure as an alert titled `title`.
fn alert(
    title: impl Into<String>,
) -> impl FnOnce(&mut Workspace, String, &mut Context<Workspace>) {
    let title = title.into();
    move |ws, error, cx| ws.apply(HostUpdate::alert(title, error), cx)
}

/// Shows each catalog the host pushes, for as long as the workspace
/// lives (ADR 0028).
fn show_catalogs(
    host: &Arc<Host>,
    workspace: &Entity<Workspace>,
    cx: &mut App,
) {
    let mut catalogs = host.follow_catalog();
    let workspace = workspace.downgrade();
    cx.spawn(async move |cx| {
        while catalogs.changed().await.is_ok() {
            let Some(catalog) = catalogs.borrow_and_update().clone() else {
                continue;
            };
            let shown = workspace.update(cx, |ws, cx| {
                ws.apply(HostUpdate::catalog(catalog), cx)
            });
            if shown.is_err() {
                return;
            }
        }
    })
    .detach();
}

/// Goes on with `run`, a finished chat, on `prompt`: it shows first,
/// so it comes before what the run does, and is taken back if the run
/// cannot go on.
fn go_on(
    host: &Arc<Host>,
    run: &RunId,
    prompt: &str,
    model: &ModelChoice,
    workspace: &Entity<Workspace>,
    cx: &mut App,
) {
    workspace.update(cx, |ws, cx| {
        let resumed = HostUpdate::Resumed {
            run: run.clone(),
            prompt: prompt.to_owned(),
            model: model.clone(),
        };
        ws.apply(resumed, cx)
    });
    let (job_run, job_prompt, job_model) =
        (run.clone(), prompt.to_owned(), model.clone());
    let (run, failed_run) = (run.clone(), run.clone());
    on_host(
        host,
        workspace,
        async move |host| {
            host.resume(&job_run, &job_prompt, &job_model).await?;
            // A message opens a closed conversation again.
            let _ = host.set_closed(&job_run, false).await;
            Ok(host.starting_of(&job_run, &job_model).await)
        },
        move |ws, starting, cx| {
            for (plugin, body) in starting {
                let run = run.clone();
                ws.apply(HostUpdate::PluginFold { run, plugin, body }, cx);
            }
        },
        move |ws, error, cx| {
            ws.apply(HostUpdate::ResumeFailed(failed_run), cx);
            ws.apply(
                HostUpdate::alert("Could not go on with the run", error),
                cx,
            )
        },
        cx,
    );
}

/// Has a model write `run`'s title from `prompt`, keeps it in the store
/// and shows it. Without a client, as in tests, or when the call fails,
/// the run keeps its placeholder. The run's record is written as it
/// starts, well before the model answers.
pub(super) fn title_in_background(
    host: &Arc<Host>,
    run: &RunId,
    prompt: &str,
    workspace: &Entity<Workspace>,
    cx: &mut App,
) {
    let Some(client) = host.client.lock().expect("not poisoned").clone() else {
        return;
    };
    let model = host.session_of(run).choice.map_or_else(
        || host.config.default_model(),
        |choice| choice.model.clone(),
    );
    let job = {
        let (run, prompt) = (run.clone(), prompt.to_owned());
        host.spawn(async move |writer| {
            let title = crate::titles::write(&client, &model, &prompt).await?;
            writer.store.set_title(&run.0, &title).await?;
            anyhow::Ok(title)
        })
    };
    let run = run.clone();
    let workspace = workspace.downgrade();
    cx.spawn(async move |cx| {
        let title = match job.await {
            Ok(Ok(title)) => title,
            Ok(Err(error)) => {
                return eprintln!(
                    "tau-ui: cannot title run {}: {error:#}",
                    run.0
                );
            }
            Err(error) => {
                return eprintln!(
                    "tau-ui: cannot title run {}: {error}",
                    run.0
                );
            }
        };
        let _ = workspace.update(cx, |ws, cx| {
            ws.apply(HostUpdate::Titled { run, title }, cx)
        });
    })
    .detach();
}

/// Shows what a drain of a main chat's queue did (ADR 0024): each
/// landing, or why a chat could not land, the queue and the conflicts
/// on main as they are now; tells the person of conflicts still on
/// main; and starts tau's turn resolving what the last landing left.
pub(super) fn show_drain(
    host: &Arc<Host>,
    report: DrainReport,
    workspace: &Entity<Workspace>,
    cx: &mut App,
) {
    let DrainReport {
        main,
        repo,
        landed,
        failed,
        resolve,
        notify,
        queue,
        conflicts,
    } = report;
    let has_landed = !landed.is_empty();
    workspace.update(cx, |ws, cx| {
        for (run, landing) in landed {
            ws.apply(
                HostUpdate::Landed {
                    run,
                    landing: Ok(landing),
                },
                cx,
            );
        }
        for (run, error) in failed {
            ws.apply(
                HostUpdate::Landed {
                    run,
                    landing: Err(error),
                },
                cx,
            );
        }
        ws.apply(
            HostUpdate::LandingQueue {
                main: main.clone(),
                queue,
                conflicts,
            },
            cx,
        );
    });
    // Landing on a main chat moves trunk: what it would push to GitHub
    // changes too.
    if has_landed {
        host.catalog_changed();
    }
    if let (Some(files), Some(hook)) = (notify, host.conflicts_hook.clone()) {
        hook(&main, &repo, &files, cx);
    }
    if let Some(prompt) = resolve {
        resolve_main(host, &main, prompt, workspace, cx);
    }
}

/// Runs `job`, which drains a main chat's queue, on the host, then shows
/// what it did; a failure is an alert titled `failed`.
fn drain_on_host<F>(
    host: &Arc<Host>,
    workspace: &Entity<Workspace>,
    job: impl FnOnce(Arc<Host>) -> F + Send + 'static,
    failed: &'static str,
    cx: &mut App,
) where
    F: Future<Output = anyhow::Result<DrainReport>> + Send + 'static,
{
    let shower = host.clone();
    on_host(
        host,
        workspace,
        job,
        move |_, report, cx| {
            let workspace = cx.entity();
            cx.defer(move |cx| show_drain(&shower, report, &workspace, cx));
        },
        alert(failed),
        cx,
    );
}

/// Starts tau's turn on `main` resolving its conflicts with `prompt`.
/// One that does not start leaves main idle with the conflicts, which
/// mark it.
fn resolve_main(
    host: &Arc<Host>,
    main: &RunId,
    prompt: String,
    workspace: &Entity<Workspace>,
    cx: &mut App,
) {
    let shower = host.clone();
    let failed_main = main.clone();
    tau_turn(
        host,
        main,
        prompt,
        async move |host, run, prompt| {
            host.start_resolving(&run, &prompt).await
        },
        "Could not start resolving the conflicts",
        move |cx| {
            drain_on_host(
                &shower,
                &cx.entity(),
                async move |host| host.resolving_failed(&failed_main).await,
                "Could not check main for conflicts",
                cx,
            )
        },
        workspace,
        cx,
    );
}

/// Starts a turn of `run` that tau asks for, with `prompt`: its chat
/// shows the message as tau's, then `start` resumes it on the host. A
/// failure says `failed` and why, then calls `then_failed`.
#[allow(clippy::too_many_arguments)]
pub(super) fn tau_turn<F>(
    host: &Arc<Host>,
    run: &RunId,
    prompt: String,
    start: impl FnOnce(Arc<Host>, RunId, String) -> F + Send + 'static,
    failed: &str,
    then_failed: impl FnOnce(&mut Context<Workspace>) + 'static,
    workspace: &Entity<Workspace>,
    cx: &mut App,
) where
    F: Future<Output = anyhow::Result<()>> + Send + 'static,
{
    workspace.update(cx, |ws, cx| {
        ws.apply(
            HostUpdate::TauTurn {
                run: run.clone(),
                prompt: prompt.clone(),
            },
            cx,
        )
    });
    let (job_run, failed_run, failed) =
        (run.clone(), run.clone(), failed.to_owned());
    on_host(
        host,
        workspace,
        move |host| start(host, job_run, prompt),
        |_, (), _| {},
        move |ws, error, cx| {
            ws.apply(HostUpdate::ResumeFailed(failed_run), cx);
            ws.apply(HostUpdate::alert(failed, error), cx);
            then_failed(cx);
        },
        cx,
    );
}

/// Updates repository `name` on the host, saying so in the status bar.
/// An update the user asked for says what went wrong in a dialog; one
/// at startup only in the status bar.
pub(super) fn update_in_background(
    host: &Arc<Host>,
    name: &str,
    workspace: &Entity<Workspace>,
    asked: bool,
    cx: &mut App,
) {
    {
        let mut updating = host.updating.lock().expect("not poisoned");
        if updating.iter().any(|repo| repo == name) {
            return;
        }
        updating.push(name.to_owned());
    }
    host.catalog_changed();
    let counted = host.job();
    let job = {
        let name = name.to_owned();
        host.spawn(async move |updater| {
            let result = updater.update_repo(&name).await;
            updater
                .updating
                .lock()
                .expect("not poisoned")
                .retain(|repo| *repo != name);
            let summary = match &result {
                Ok(updated) if updated.changed() => format!(
                    "{name} updated to {}",
                    updated.after.get(..7).unwrap_or(&updated.after)
                ),
                Ok(_) => format!("{name} is up to date"),
                Err(_) => format!("{name} was not updated"),
            };
            *updater.last_update.lock().expect("not poisoned") = Some(summary);
            updater.catalog_changed();
            result.map(drop)
        })
    };
    let name = name.to_owned();
    let workspace = workspace.downgrade();
    cx.spawn(async move |cx| {
        let _counted = counted;
        let Ok(result) = job.await else {
            return;
        };
        let _ = workspace.update(cx, |ws, cx| {
            if let (true, Err(error)) = (asked, result) {
                ws.apply(
                    HostUpdate::alert(
                        format!("Could not update {name}"),
                        format!("{error:#}"),
                    ),
                    cx,
                );
            }
        });
    })
    .detach();
}

/// Drains every main chat's queue on the host, as tau starts, and shows
/// each.
fn drain_all_in_background(
    host: &Arc<Host>,
    workspace: &Entity<Workspace>,
    cx: &mut App,
) {
    let job = host.spawn(async move |drainer| drainer.drain_all().await);
    let (host, workspace) = (host.clone(), workspace.downgrade());
    cx.spawn(async move |cx| {
        let Ok(reports) = job.await else {
            return;
        };
        let Some(workspace) = workspace.upgrade() else {
            return;
        };
        cx.update(|cx| {
            for report in reports {
                show_drain(&host, report, &workspace, cx);
            }
        });
    })
    .detach();
}

/// Finishes what the last tau left as it closed (`Host::recover`) on
/// the host, then brings in what changed upstream while it was closed,
/// quietly: an update moves trunk, which recovery reads.
fn recover_in_background(
    host: &Arc<Host>,
    slots: Vec<RepoSlot>,
    workspace: &Entity<Workspace>,
    cx: &mut App,
) {
    // The repositories as they are, before recovery and updates.
    if !slots.is_empty() {
        host.catalog_changed();
    }
    let counted = host.job();
    let job = host.spawn(async move |recoverer| recoverer.recover().await);
    let host = host.clone();
    let workspace = workspace.downgrade();
    cx.spawn(async move |cx| {
        let _counted = counted;
        let finished = match job.await {
            Ok(Ok(finished)) => finished,
            Ok(Err(error)) => {
                eprintln!("tau-ui: cannot finish what tau left: {error:#}");
                Vec::new()
            }
            Err(error) => {
                eprintln!("tau-ui: cannot finish what tau left: {error}");
                Vec::new()
            }
        };
        let Some(workspace) = workspace.upgrade() else {
            return;
        };
        cx.update(|cx| {
            workspace.update(cx, |ws, cx| {
                for record in finished {
                    ws.apply(HostUpdate::LandingFinished(record), cx);
                }
            });
            // What waited to land when tau closed lands now, and the
            // conflicts on each main chat are read again (ADR 0024).
            drain_all_in_background(&host, &workspace, cx);
            for slot in &slots {
                update_in_background(&host, &slot.name, &workspace, false, cx);
            }
        });
    })
    .detach();
}

/// Starts a run with `start` on the host, then shows it and has its
/// title written; a failure is an alert titled `failed`.
fn start_on_host<F>(
    host: &Arc<Host>,
    prompt: &str,
    start: impl FnOnce(Arc<Host>) -> F + Send + 'static,
    failed: &'static str,
    workspace: &Entity<Workspace>,
    cx: &mut App,
) where
    F: Future<Output = anyhow::Result<RunView>> + Send + 'static,
{
    let (titler, prompt) = (host.clone(), prompt.to_owned());
    on_host(
        host,
        workspace,
        start,
        move |ws, view, cx| {
            let run = view.id.clone();
            ws.apply(HostUpdate::Run(Box::new(view)), cx);
            let workspace = cx.entity();
            cx.defer(move |cx| {
                title_in_background(&titler, &run, &prompt, &workspace, cx)
            });
        },
        alert(failed),
        cx,
    );
}

impl Host {
    /// Wires the host to a workspace: its events drive the host, and the
    /// runs' events drive the workspace. Loads history first.
    pub fn attach(
        self,
        workspace: &Entity<Workspace>,
        mut events: mpsc::UnboundedReceiver<RunEvent>,
        cx: &mut App,
    ) -> Arc<Host> {
        let host = Arc::new(self);
        let attached = host.clone();
        show_catalogs(&host, workspace, cx);
        {
            let history = host.spawn(async move |host| host.history().await);
            let workspace = workspace.downgrade();
            cx.spawn(async move |cx| match history.await {
                Ok(Ok(runs)) => {
                    let _ = workspace.update(cx, |ws, cx| {
                        ws.apply(HostUpdate::History(runs), cx)
                    });
                }
                Ok(Err(error)) => {
                    eprintln!("tau-ui: cannot read past runs: {error:#}")
                }
                Err(error) => {
                    eprintln!("tau-ui: cannot read past runs: {error}")
                }
            })
            .detach();
        }
        // Finished forks say whether they would land cleanly.
        super::forecast::follow(&host, workspace, cx);
        // Phones reach this host once allowed.
        crate::phone_server::serve(
            host.runtime.handle().clone(),
            host.config.credentials.dir.join("phones"),
            workspace,
            cx,
        );
        // Once a repository is imported, the plugins and the status bar
        // change: the host pushes its catalog then.
        let slots = host.repos.lock().expect("not poisoned").clone();
        recover_in_background(&host, slots, workspace, cx);
        github::restore(workspace, &host.config.credentials, &host.github, cx);
        let handler = host.clone();
        // A sign-in, a switch of account or a sign-out from the Models
        // screen changes what new runs use.
        let connected: accounts::Connected = {
            let (host, entity) = (host.clone(), workspace.downgrade());
            std::rc::Rc::new(move |account, cx| {
                let applied = host.set_account(account);
                let Some(workspace) = entity.upgrade() else {
                    return;
                };
                check_eligibility(&host, &workspace, cx);
                host.catalog_changed();
                if let Err(error) = applied {
                    workspace.update(cx, |ws, cx| {
                        ws.apply(
                            HostUpdate::alert(
                                "Could not use the new sign-in",
                                format!("{error:#}"),
                            ),
                            cx,
                        );
                    });
                }
            })
        };
        let sign_ins = accounts::SignIns::default();
        cx.subscribe(
            workspace,
            move |workspace, event: &WorkspaceEvent, cx| {
                if sign_ins.handle(
                    event,
                    &workspace,
                    &handler.config.credentials,
                    handler.config.model.as_deref(),
                    &connected,
                    cx,
                ) || github::handle(
                    event,
                    &workspace,
                    &handler.config.credentials,
                    &handler.github,
                    cx,
                ) {
                    return;
                }
                match event {
                WorkspaceEvent::UpdateRepo { repo } => {
                    update_in_background(&handler, repo, &workspace, true, cx)
                }
                WorkspaceEvent::CloneRepos { repos } => {
                    for name in repos {
                        clone_into_tau(&handler, name, &workspace, cx);
                    }
                }
                WorkspaceEvent::NewRun {
                    prompt,
                    model,
                    repo,
                } => {
                    let (job_prompt, model, repo) =
                        (prompt.clone(), model.clone(), repo.clone());
                    start_on_host(
                        &handler,
                        prompt,
                        async move |host| {
                            host.start(&job_prompt, &model, &repo).await
                        },
                        "Could not start the run",
                        &workspace,
                        cx,
                    );
                }
                WorkspaceEvent::PreparePullRequest { run } => {
                    let (job_run, run) = (run.clone(), run.clone());
                    on_host(
                        &handler,
                        &workspace,
                        async move |host| host.prepare_pull_request(&job_run).await,
                        move |ws, draft, cx| {
                            let pr = Box::new(draft);
                            ws.apply(HostUpdate::PullRequest { run, pr }, cx)
                        },
                        |ws, error, cx| {
                            ws.back(cx);
                            alert("Could not write the pull request")(ws, error, cx)
                        },
                        cx,
                    );
                }
                WorkspaceEvent::CreatePullRequest {
                    run,
                    title,
                    body,
                    draft,
                    keep_pushing,
                    reviewers,
                } => {
                    let Some(prepared) = handler.draft(run) else {
                        return;
                    };
                    let job = {
                        let (run, title, body, reviewers) =
                            (run.clone(), title.clone(), body.clone(), reviewers.clone());
                        let (draft, keep_pushing) = (*draft, *keep_pushing);
                        handler.spawn(async move |host| {
                            host.create_pull_request(
                                &run,
                                &prepared,
                                &title,
                                &body,
                                draft,
                                keep_pushing,
                                &reviewers,
                            )
                            .await
                        })
                    };
                    let (host, run, workspace) =
                        (handler.clone(), run.clone(), workspace.downgrade());
                    cx.spawn(async move |cx| {
                        let created = match job.await {
                            Ok(result) => result.map_err(|error| format!("{error:#}")),
                            Err(error) => Err(error.to_string()),
                        };
                        let opened = created.is_ok();
                        let state = match created {
                            Ok((opened, _)) => PrState::Opened {
                                number: opened.number,
                                url: opened.url,
                                checks: crate::pull_request::Checks::Running,
                            },
                            Err(error) => PrState::Failed(error),
                        };
                        let _ = workspace.update(cx, |ws, cx| {
                            ws.apply(HostUpdate::PullRequestState { run: run.clone(), state }, cx)
                        });
                        if opened {
                            watch_checks(host, run, workspace, cx).await;
                        }
                    })
                    .detach();
                }
                WorkspaceEvent::Push { repo, fetch } => {
                    let (job_repo, fetch) = (repo.clone(), *fetch);
                    let job = handler.spawn(async move |host| {
                        let result = host.push_main(&job_repo, fetch).await;
                        // The count ahead of GitHub, and trunk after a
                        // fetch, change with it.
                        host.catalog_changed();
                        result
                    });
                    let (repo, workspace) = (repo.clone(), workspace.downgrade());
                    cx.spawn(async move |cx| {
                        let result = job.await.unwrap_or_else(|error| {
                            Err(crate::push::PushFailure::Failed(error.to_string()))
                        });
                        let _ = workspace.update(cx, |ws, cx| {
                            ws.apply(HostUpdate::Pushed { repo, result }, cx);
                        });
                    })
                    .detach();
                }
                WorkspaceEvent::Query { sql } => {
                    let sql = sql.clone();
                    on_host(
                        &handler,
                        &workspace,
                        async move |host| Ok(host.store.query(&sql, 200).await?),
                        |ws, table, cx| ws.apply(HostUpdate::QueryResult(Ok(table)), cx),
                        |ws, error, cx| ws.apply(HostUpdate::QueryResult(Err(error)), cx),
                        cx,
                    );
                }
                WorkspaceEvent::JevKey { key } => {
                    let key = key.clone();
                    on_host(
                        &handler,
                        &workspace,
                        async move |host| {
                            let saved = host.set_jev_key(key.as_deref()).await;
                            host.catalog_changed();
                            saved
                        },
                        |_, (), _| {},
                        alert("Could not save the TypeSafe key"),
                        cx,
                    );
                }
                WorkspaceEvent::PluginAct { plugin, action } => {
                    // On the host: an action may ask Jev, or the store.
                    let (job_plugin, plugin, action) =
                        (plugin.clone(), plugin.clone(), action.clone());
                    let failed = alert(format!("{plugin} could not do that"));
                    on_host(
                        &handler,
                        &workspace,
                        async move |host| host.plugin_act(&job_plugin, action).await,
                        move |ws, reply, cx| {
                            if let Some(reply) = reply {
                                ws.apply(HostUpdate::PluginReply { plugin, reply }, cx)
                            }
                        },
                        failed,
                        cx,
                    );
                }
                WorkspaceEvent::PluginSettings {
                    plugin,
                    repo,
                    settings,
                } => {
                    let (job_plugin, repo, settings) =
                        (plugin.clone(), repo.clone(), settings.clone());
                    on_host(
                        &handler,
                        &workspace,
                        async move |host| {
                            let saved =
                                host.save_plugin_settings(&job_plugin, repo, settings).await;
                            host.catalog_changed();
                            saved
                        },
                        |_, (), _| {},
                        alert(format!("Could not save {plugin}'s settings")),
                        cx,
                    );
                }
                WorkspaceEvent::PluginRecord { run, plugin, body } => {
                    let (job_run, job_plugin, job_body) =
                        (run.clone(), plugin.clone(), body.clone());
                    let (run, plugin, body) = (run.clone(), plugin.clone(), body.clone());
                    let failed = alert(format!("Could not save what {plugin} changed"));
                    on_host(
                        &handler,
                        &workspace,
                        async move |host| {
                            host.store_plugin_record(&job_run, &job_plugin, &job_body)
                                .await
                        },
                        move |ws, (), cx| {
                            ws.apply(HostUpdate::PluginRecord { run, plugin, body }, cx)
                        },
                        failed,
                        cx,
                    );
                }
                WorkspaceEvent::CloseRun { run } => {
                    let job_run = run.clone();
                    handler.spawn(async move |host| {
                        if let Err(error) = host.set_closed(&job_run, true).await {
                            eprintln!("tau-ui: cannot save closed runs: {error:#}");
                        }
                    });
                    workspace.update(cx, |ws, cx| ws.apply(HostUpdate::Closed(run.clone()), cx));
                }
                WorkspaceEvent::Say { run, text, model } => {
                    let (job_run, job_text) = (run.clone(), text.clone());
                    let (run, text, model) = (run.clone(), text.clone(), model.clone());
                    let goer = handler.clone();
                    on_host(
                        &handler,
                        &workspace,
                        async move |host| host.steer(&job_run, &job_text).await,
                        move |ws, delivery, cx| match delivery {
                            Delivery::Steered => {
                                ws.apply(HostUpdate::Steered { run, text }, cx);
                            }
                            Delivery::GoOn => {
                                let workspace = cx.entity();
                                cx.defer(move |cx| {
                                    go_on(&goer, &run, &text, &model, &workspace, cx)
                                });
                            }
                            // The sub-agent ended before reading it: main
                            // has it, in its turn or in one tau starts.
                            Delivery::ToMain { main, text, steered } => {
                                ws.apply(
                                    HostUpdate::alert(
                                        "Sent to main",
                                        "The sub-agent had finished, so its main chat got your message.",
                                    ),
                                    cx,
                                );
                                if steered {
                                    // Main shows it queued, as any steer,
                                    // until its turn reads it.
                                    ws.apply(HostUpdate::Steered { run: main, text }, cx);
                                } else {
                                    // Main was idle: tau's turn on it, as
                                    // for a report.
                                    let workspace = cx.entity();
                                    cx.defer(move |cx| {
                                        resolve_main(&goer, &main, text, &workspace, cx)
                                    });
                                }
                            }
                        },
                        alert("Could not send the message"),
                        cx,
                    );
                }
                WorkspaceEvent::ResumeCutOff { run } => tau_turn(
                    &handler,
                    run,
                    CUT_OFF.to_owned(),
                    async move |host, run, _| host.resume_cut_off(&run).await,
                    "Could not resume the run",
                    |_| {},
                    &workspace,
                    cx,
                ),
                WorkspaceEvent::Fork {
                    run,
                    turn,
                    prompt,
                    model,
                } => {
                    let (run, turn, job_prompt, model) =
                        (run.clone(), *turn, prompt.clone(), model.clone());
                    start_on_host(
                        &handler,
                        prompt,
                        async move |host| {
                            host.fork(&run, turn, &job_prompt, &model).await
                        },
                        "Could not fork the run",
                        &workspace,
                        cx,
                    );
                }
                WorkspaceEvent::CompareCode { main, fork } => {
                    let code = |ws: &mut Workspace, main, fork, code, cx: &mut Context<Workspace>| {
                        ws.apply(HostUpdate::BranchCode { main, fork, code }, cx)
                    };
                    let (job_main, job_fork) = (main.clone(), fork.clone());
                    let (main, fork) = (main.clone(), fork.clone());
                    let (failed_main, failed_fork) = (main.clone(), fork.clone());
                    on_host(
                        &handler,
                        &workspace,
                        async move |host| host.branch_code(&job_main, &job_fork).await,
                        move |ws, ready, cx| code(ws, main, fork, CodeState::Ready(ready), cx),
                        move |ws, error, cx| {
                            code(ws, failed_main, failed_fork, CodeState::Unavailable(error), cx)
                        },
                        cx,
                    );
                }
                WorkspaceEvent::SetDefaultModel { .. } | WorkspaceEvent::HideModel { .. } => {
                    // The change, to the settings as kept now: another
                    // interface may have changed others since.
                    let event = event.clone();
                    on_host(
                        &handler,
                        &workspace,
                        async move |host| {
                            let saved = host
                                .change_settings(|settings| match &event {
                                    WorkspaceEvent::SetDefaultModel { agent, choice } => {
                                        settings.set_default(agent, choice.clone())
                                    }
                                    WorkspaceEvent::HideModel { id, hidden } => {
                                        settings.set_hidden(id, *hidden)
                                    }
                                    _ => {}
                                })
                                .await;
                            // What is saved, for every interface; the one
                            // that changed it showed the change already.
                            host.catalog_changed();
                            saved
                        },
                        |_, (), _| {},
                        alert("Could not save the model settings"),
                        cx,
                    );
                }
                WorkspaceEvent::PreviewLanding { run } => {
                    let preview = |ws: &mut Workspace, run, preview, cx: &mut Context<Workspace>| {
                        ws.apply(HostUpdate::LandingPreview { run, preview }, cx)
                    };
                    let (job_run, run, failed_run) = (run.clone(), run.clone(), run.clone());
                    on_host(
                        &handler,
                        &workspace,
                        async move |host| host.preview_landing(&job_run).await,
                        move |ws, landing, cx| preview(ws, run, Ok(landing), cx),
                        move |ws, error, cx| preview(ws, failed_run, Err(error), cx),
                        cx,
                    );
                }
                // A chat lands by joining its main chat's queue, which
                // lands it at once when main is idle and nothing waits
                // before it (ADR 0024). What conflicts, main resolves,
                // in a turn tau starts (ADR 0014).
                WorkspaceEvent::Land { run } => {
                    let (job_run, failed_run) = (run.clone(), run.clone());
                    let shower = handler.clone();
                    on_host(
                        &handler,
                        &workspace,
                        async move |host| host.queue_landing(&job_run).await,
                        move |_, report, cx| {
                            let workspace = cx.entity();
                            cx.defer(move |cx| show_drain(&shower, report, &workspace, cx));
                        },
                        move |ws, error, cx| {
                            ws.apply(HostUpdate::Landed { run: failed_run, landing: Err(error) }, cx)
                        },
                        cx,
                    );
                }
                WorkspaceEvent::Unqueue { run } => {
                    let run = run.clone();
                    drain_on_host(
                        &handler,
                        &workspace,
                        async move |host| host.unqueue(&run).await,
                        "Could not take the chat out of the queue",
                        cx,
                    );
                }
                WorkspaceEvent::DismissConflicts { main } => {
                    let main = main.clone();
                    drain_on_host(
                        &handler,
                        &workspace,
                        async move |host| host.dismiss_conflicts(&main).await,
                        "Could not save that",
                        cx,
                    );
                }
                WorkspaceEvent::ResolveAgain { main } => {
                    let (job_main, main) = (main.clone(), main.clone());
                    let resolver = handler.clone();
                    on_host(
                        &handler,
                        &workspace,
                        async move |host| host.resolve_again_prompt(&job_main).await,
                        move |_, prompt, cx| {
                            let workspace = cx.entity();
                            cx.defer(move |cx| {
                                resolve_main(&resolver, &main, prompt, &workspace, cx)
                            });
                        },
                        alert("Could not resolve again"),
                        cx,
                    );
                }
                WorkspaceEvent::DropChild { run } => {
                    // A dropped chat leaves the queue it waited in.
                    let (job_run, run) = (run.clone(), run.clone());
                    let failed_run = run.clone();
                    let shower = handler.clone();
                    on_host(
                        &handler,
                        &workspace,
                        async move |host| {
                            host.drop_child(&job_run).await?;
                            host.unqueue(&job_run).await
                        },
                        move |ws, report, cx| {
                            ws.apply(HostUpdate::Dropped { run, result: Ok(()) }, cx);
                            let workspace = cx.entity();
                            cx.defer(move |cx| show_drain(&shower, report, &workspace, cx));
                        },
                        move |ws, error, cx| {
                            ws.apply(HostUpdate::Dropped { run: failed_run, result: Err(error) }, cx)
                        },
                        cx,
                    );
                }
                WorkspaceEvent::KeepBranch { run } => {
                    let (job_run, run) = (run.clone(), run.clone());
                    on_host(
                        &handler,
                        &workspace,
                        async move |host| host.keep_branch(&job_run).await,
                        move |ws, (), cx| ws.apply(HostUpdate::BranchKept(run), cx),
                        alert("Could not drop the other branches"),
                        cx,
                    );
                }
                WorkspaceEvent::HideRepo { repo } => {
                    // The list without it, for every interface; the one
                    // that hid it showed that already.
                    let repo = repo.clone();
                    on_host(
                        &handler,
                        &workspace,
                        async move |host| {
                            let hidden = host.hide_repo(&repo).await;
                            host.catalog_changed();
                            hidden
                        },
                        |_, (), _| {},
                        alert("Could not save the repository list"),
                        cx,
                    );
                }
                WorkspaceEvent::OpenRepos(open) => {
                    let open = open.clone();
                    handler.spawn(async move |host| {
                        if let Err(error) = host.set_open_repos(open).await {
                            eprintln!(
                                "tau-ui: cannot save the repository list: {error:#}"
                            );
                        }
                    });
                }
                // `phone_server::serve` handles these in its own
                // subscription.
                WorkspaceEvent::Phones(_) => {}
                WorkspaceEvent::Cancel { run } => handler.cancel(run),
                other => eprintln!("tau-ui: not handled yet: {other:?}"),
                }
            },
        )
        .detach();
        // What plugins' host halves tell the interface.
        if let Some(mut pushed) =
            host.pushed.lock().expect("not poisoned").take()
        {
            let (host, workspace) = (host.clone(), workspace.downgrade());
            cx.spawn(async move |cx| {
                while let Some(push) = pushed.recv().await {
                    let Some(workspace) = workspace.upgrade() else {
                        return;
                    };
                    cx.update(|cx| {
                        hosted::apply_push(&host, push, &workspace, cx)
                    });
                }
            })
            .detach();
        }
        let workspace = workspace.downgrade();
        cx.spawn(async move |cx| {
            while let Some(event) = events.recv().await {
                // The run may have asked Jev: the Plugins screen's count
                // follows.
                if matches!(event, RunEvent::RunEnd { .. }) {
                    host.catalog_changed();
                }
                // A pull request that keeps pushing takes the commits the
                // turn made.
                if let RunEvent::TurnEnd { run, .. } = &event
                    && host.keeps_pushing(run)
                {
                    let run = run.clone();
                    host.spawn(async move |pusher| {
                        if let Err(error) =
                            pusher.push_later_commits(&run).await
                        {
                            eprintln!(
                                "tau-ui: cannot push the turn: {error:#}"
                            );
                        }
                    });
                }
                // A run the ChatGPT plan stopped says what to do next.
                let refusal = match &event {
                    RunEvent::RunEnd {
                        stop: StopReason::Error(_),
                        ..
                    } => host.refusal(),
                    _ => None,
                };
                if let RunEvent::Steered { run, text } = &event {
                    host.steer_read(run, text);
                }
                // What the run was steered with too late to read: a run
                // that stopped on its own goes on with it; a cancelled one
                // was stopped on purpose.
                // A sub-agent does not go on: it lands, or is dropped, and
                // what it did not read goes to main (ADR 0026).
                let sub_agent_ended = match &event {
                    RunEvent::RunEnd { run, .. } if host.is_sub_agent(run) => {
                        Some((run.clone(), host.take_unread(run)))
                    }
                    _ => None,
                };
                let unread = match &event {
                    RunEvent::RunEnd { run, .. } if host.is_sub_agent(run) => {
                        None
                    }
                    RunEvent::RunEnd { run, stop, .. } => {
                        let unread = host.take_unread(run);
                        (*stop != StopReason::Cancelled && !unread.is_empty())
                            .then(|| (run.clone(), unread.join("\n\n")))
                    }
                    _ => None,
                };
                // A main chat's turn ended: what it left in conflict
                // marks it, and what waited lands (ADR 0024).
                // Stopped by the person, nothing lands on it yet, and tau
                // starts no turn on it: Stop means stop (ADR 0026).
                let main_ended = match &event {
                    RunEvent::RunEnd { run, stop, .. } if host.is_main(run) => {
                        Some((run.clone(), *stop == StopReason::Cancelled))
                    }
                    _ => None,
                };
                // A chat's turn ended as it meant to: a plugin may let it
                // land by itself and go on (ADR 0034).
                let chat_ended = match &event {
                    RunEvent::RunEnd {
                        run,
                        stop: StopReason::Stop,
                        ..
                    } if !host.is_main(run) && !host.is_sub_agent(run) => {
                        Some(run.clone())
                    }
                    _ => None,
                };
                let applied = workspace.update(cx, |ws, cx| {
                    ws.apply_streamed(event.clone(), cx);
                    if let Some(refusal) = &refusal {
                        ws.apply(HostUpdate::PlanRefusal(refusal.clone()), cx);
                    }
                });
                if applied.is_err() {
                    break;
                }
                if let (Some((run, text)), Some(entity)) =
                    (unread, workspace.upgrade())
                {
                    let (host, model) = (host.clone(), host.choice_of(&run));
                    cx.update(|cx| {
                        go_on(&host, &run, &text, &model, &entity, cx)
                    });
                }
                // A sub-agent of a main chat ended: nobody waiting, it
                // lands on main and tau reports it (ADR 0026).
                if let (Some((child, unread)), Some(entity)) =
                    (sub_agent_ended, workspace.upgrade())
                {
                    let host = host.clone();
                    cx.update(|cx| {
                        drain_on_host(
                            &host,
                            &entity,
                            async move |host| {
                                host.sub_agent_ended(&child, unread).await
                            },
                            "Could not land the sub-agent's work",
                            cx,
                        )
                    });
                }
                if let (Some((main, stopped)), Some(entity)) =
                    (main_ended, workspace.upgrade())
                {
                    let host = host.clone();
                    cx.update(|cx| {
                        drain_on_host(
                            &host,
                            &entity,
                            async move |host| {
                                if stopped {
                                    host.main_turn_stopped(&main).await
                                } else {
                                    host.main_turn_ended(&main).await
                                }
                            },
                            "Could not land what waits on main",
                            cx,
                        )
                    });
                }
                if let (Some(chat), Some(entity)) =
                    (chat_ended, workspace.upgrade())
                {
                    let shower = host.clone();
                    cx.update(|cx| {
                        on_host(
                            &host,
                            &entity,
                            async move |host| host.land_itself(&chat).await,
                            move |ws, record, cx| {
                                if let Some(record) = record {
                                    ws.apply(
                                        HostUpdate::LandedItself(record),
                                        cx,
                                    );
                                    // It moved trunk.
                                    shower.catalog_changed();
                                }
                            },
                            |_, error, _| {
                                eprintln!(
                                    "tau-ui: a chat could not land by \
                                     itself: {error}"
                                )
                            },
                            cx,
                        )
                    });
                }
            }
            // Keep the host, and its runtime, alive as long as events flow.
            drop(host);
        })
        .detach();
        attached
    }
}
