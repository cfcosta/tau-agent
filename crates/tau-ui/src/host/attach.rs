//! The interface asking the host for something, and the host answering
//! it off the interface's thread.

use gpui::Context;

use super::*;

/// Runs `job` on the host's blocking pool, off the interface's thread,
/// then hands what it gave to the workspace: `done` with its value, or
/// `failed` with what went wrong.
fn off_thread<T: Send + 'static>(
    host: &Arc<Host>,
    workspace: &Entity<Workspace>,
    job: impl FnOnce(&Host) -> anyhow::Result<T> + Send + 'static,
    done: impl FnOnce(&mut Workspace, T, &mut Context<Workspace>) + 'static,
    failed: impl FnOnce(&mut Workspace, String, &mut Context<Workspace>) + 'static,
    cx: &mut App,
) {
    let job = {
        let worker = host.clone();
        host.runtime.spawn_blocking(move || job(&worker))
    };
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
        let (writer, run, prompt) =
            (host.clone(), run.clone(), prompt.to_owned());
        host.runtime.spawn(async move {
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

/// Shows what a drain of a main chat's queue did (ADR 0021): each
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
        let catalog = host.catalog();
        workspace
            .update(cx, |ws, cx| ws.apply(HostUpdate::catalog(catalog), cx));
    }
    if let (Some(files), Some(hook)) = (notify, host.conflicts_hook.clone()) {
        hook(&main, &repo, &files, cx);
    }
    if let Some(prompt) = resolve {
        resolve_main(host, &main, prompt, workspace, cx);
    }
}

/// Runs `job`, which drains a main chat's queue, off the interface's
/// thread, then shows what it did; a failure is an alert titled
/// `failed`.
fn drain_off_thread(
    host: &Arc<Host>,
    workspace: &Entity<Workspace>,
    job: impl FnOnce(&Host) -> anyhow::Result<DrainReport> + Send + 'static,
    failed: &'static str,
    cx: &mut App,
) {
    let shower = host.clone();
    off_thread(
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
    let started = std::cell::Cell::new(true);
    tau_turn(
        host,
        main,
        prompt,
        |host, run, prompt| {
            let result = host.start_resolving(run, prompt);
            started.set(result.is_ok());
            result
        },
        "Could not start resolving the conflicts",
        workspace,
        cx,
    );
    if !started.get() {
        let main = main.clone();
        drain_off_thread(
            host,
            workspace,
            move |host| host.resolving_failed(&main),
            "Could not check main for conflicts",
            cx,
        );
    }
}

/// Starts a turn of `run` that tau asks for, with `prompt`: its chat
/// shows the message as tau's, then `start` resumes it. A failure says
/// `failed` and why.
pub(super) fn tau_turn(
    host: &Arc<Host>,
    run: &RunId,
    prompt: String,
    start: impl FnOnce(&Host, &RunId, &str) -> anyhow::Result<()>,
    failed: &str,
    workspace: &Entity<Workspace>,
    cx: &mut App,
) {
    workspace.update(cx, |ws, cx| {
        ws.apply(
            HostUpdate::TauTurn {
                run: run.clone(),
                prompt: prompt.clone(),
            },
            cx,
        )
    });
    if let Err(error) = start(host, run, &prompt) {
        workspace.update(cx, |ws, cx| {
            ws.apply(HostUpdate::ResumeFailed(run.clone()), cx);
            ws.apply(HostUpdate::alert(failed, format!("{error:#}")), cx);
        });
    }
}

/// Updates repository `name` off the UI thread, saying so in the status
/// bar. An update the user asked for says what went wrong in a dialog;
/// one at startup only in the status bar.
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
    let catalog = host.catalog();
    workspace.update(cx, |ws, cx| ws.apply(HostUpdate::catalog(catalog), cx));
    let job = {
        let (updater, name) = (host.clone(), name.to_owned());
        host.runtime
            .spawn_blocking(move || updater.update_repo(&name))
    };
    let (host, name) = (host.clone(), name.to_owned());
    let workspace = workspace.downgrade();
    cx.spawn(async move |cx| {
        let result = match job.await {
            Ok(result) => result.map_err(|error| format!("{error:#}")),
            Err(error) => Err(error.to_string()),
        };
        host.updating
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
        *host.last_update.lock().expect("not poisoned") = Some(summary);
        let catalog = host.catalog();
        let _ = workspace.update(cx, |ws, cx| {
            ws.apply(HostUpdate::catalog(catalog), cx);
            if let (true, Err(error)) = (asked, result) {
                ws.apply(
                    HostUpdate::alert(
                        format!("Could not update {name}"),
                        error,
                    ),
                    cx,
                );
            }
        });
    })
    .detach();
}

/// Drains every main chat's queue off the interface's thread, as tau
/// starts, and shows each.
fn drain_all_off_thread(
    host: &Arc<Host>,
    workspace: &Entity<Workspace>,
    cx: &mut App,
) {
    let job = {
        let drainer = host.clone();
        host.runtime.spawn_blocking(move || drainer.drain_all())
    };
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

/// Finishes what the last tau left as it closed (`Host::recover`), off
/// the interface's thread, then brings in what changed upstream while it
/// was closed, quietly: an update moves trunk, which recovery reads.
fn recover_in_background(
    host: &Arc<Host>,
    slots: Vec<RepoSlot>,
    workspace: &Entity<Workspace>,
    cx: &mut App,
) {
    // The repositories as they are, before recovery and updates.
    if !slots.is_empty() {
        let catalog = host.catalog();
        workspace
            .update(cx, |ws, cx| ws.apply(HostUpdate::catalog(catalog), cx));
    }
    let job = {
        let recoverer = host.clone();
        host.runtime.spawn_blocking(move || recoverer.recover())
    };
    let host = host.clone();
    let workspace = workspace.downgrade();
    cx.spawn(async move |cx| {
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
            // conflicts on each main chat are read again (ADR 0021).
            let host_job = host.clone();
            drain_all_off_thread(&host_job, &workspace, cx);
            for slot in &slots {
                update_in_background(&host, &slot.name, &workspace, false, cx);
            }
        });
    })
    .detach();
}

impl Host {
    /// Wires the host to a workspace: its events drive the host, and the
    /// runs' events drive the workspace. Loads history first.
    pub fn attach(
        self,
        workspace: &Entity<Workspace>,
        mut events: mpsc::UnboundedReceiver<RunEvent>,
        cx: &mut App,
    ) {
        match self.history() {
            Ok(runs) => workspace
                .update(cx, |ws, cx| ws.apply(HostUpdate::History(runs), cx)),
            Err(error) => eprintln!("tau-ui: cannot read past runs: {error:#}"),
        }
        let host = Arc::new(self);
        // Phones reach this host once allowed.
        crate::phone_server::serve(
            host.runtime.handle().clone(),
            host.config.credentials.dir.join("phones"),
            workspace,
            cx,
        );
        // Once a repository is imported, the plugins and the status bar
        // change.
        let slots = host.repos.lock().expect("not poisoned").clone();
        for slot in &slots {
            refresh_when_imported(&host, slot, workspace, cx);
        }
        recover_in_background(&host, slots, workspace, cx);
        github::restore(workspace, &host.config.credentials, &host.github, cx);
        let handler = host.clone();
        // A sign-in, a switch of account or a sign-out from the Models
        // screen changes what new runs use.
        let connected: accounts::Connected = {
            let (host, entity) = (host.clone(), workspace.downgrade());
            std::rc::Rc::new(move |account, cx| {
                let applied = host.set_account(account);
                let catalog = host.catalog();
                let Some(workspace) = entity.upgrade() else {
                    return;
                };
                check_eligibility(&host, &workspace, cx);
                workspace.update(cx, |ws, cx| {
                    ws.apply(HostUpdate::catalog(catalog), cx);
                    if let Err(error) = applied {
                        ws.apply(
                            HostUpdate::alert(
                                "Could not use the new sign-in",
                                format!("{error:#}"),
                            ),
                            cx,
                        );
                    }
                });
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
                } => match handler.start(prompt, model, repo) {
                    Ok(view) => {
                        let run = view.id.clone();
                        workspace.update(cx, |ws, cx| ws.apply(HostUpdate::Run(Box::new(view)), cx));
                        title_in_background(&handler, &run, prompt, &workspace, cx);
                    }
                    Err(error) => workspace.update(cx, |ws, cx| {
                        ws.apply(HostUpdate::alert("Could not start the run", format!("{error:#}")), cx)
                    }),
                },
                WorkspaceEvent::PreparePullRequest { run } => {
                    let (job_run, run) = (run.clone(), run.clone());
                    off_thread(
                        &handler,
                        &workspace,
                        move |host| host.prepare_pull_request(&job_run),
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
                        let host = handler.clone();
                        let (run, title, body, reviewers) =
                            (run.clone(), title.clone(), body.clone(), reviewers.clone());
                        let (draft, keep_pushing) = (*draft, *keep_pushing);
                        handler.runtime.spawn_blocking(move || {
                            host.create_pull_request(
                                &run,
                                &prepared,
                                &title,
                                &body,
                                draft,
                                keep_pushing,
                                &reviewers,
                            )
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
                    let job = {
                        let host = handler.clone();
                        handler.runtime.spawn_blocking(move || {
                            host.push_main(&job_repo, fetch)
                        })
                    };
                    let (host, repo, workspace) =
                        (handler.clone(), repo.clone(), workspace.downgrade());
                    cx.spawn(async move |cx| {
                        let result = job.await.unwrap_or_else(|error| {
                            Err(crate::push::PushFailure::Failed(error.to_string()))
                        });
                        // The count ahead of GitHub, and trunk after a
                        // fetch, change with it.
                        let catalog = host.catalog();
                        let _ = workspace.update(cx, |ws, cx| {
                            ws.apply(HostUpdate::catalog(catalog), cx);
                            ws.apply(HostUpdate::Pushed { repo, result }, cx);
                        });
                    })
                    .detach();
                }
                WorkspaceEvent::Query { sql } => {
                    let sql = sql.clone();
                    off_thread(
                        &handler,
                        &workspace,
                        move |host| Ok(host.block_on(host.store.query(&sql, 200))?),
                        |ws, table, cx| ws.apply(HostUpdate::QueryResult(Ok(table)), cx),
                        |ws, error, cx| ws.apply(HostUpdate::QueryResult(Err(error)), cx),
                        cx,
                    );
                }
                WorkspaceEvent::JevKey { key } => {
                    let saved =
                        handler.config.credentials.set_jev_key(key.as_deref());
                    let catalog = handler.catalog();
                    workspace.update(cx, |ws, cx| {
                        ws.apply(HostUpdate::catalog(catalog), cx);
                        if let Err(error) = saved {
                            ws.apply(HostUpdate::alert("Could not save the TypeSafe key", error.to_string()), cx);
                        }
                    });
                }
                WorkspaceEvent::PluginAct { plugin, action } => {
                    // Off the UI thread: an action may ask Jev, or the
                    // store.
                    let (job_plugin, plugin, action) =
                        (plugin.clone(), plugin.clone(), action.clone());
                    let failed = alert(format!("{plugin} could not do that"));
                    off_thread(
                        &handler,
                        &workspace,
                        move |host| host.plugin_act(&job_plugin, action),
                        move |ws, reply, cx| {
                            if let Some(reply) = reply {
                                ws.apply(HostUpdate::PluginReply { plugin, reply }, cx)
                            }
                        },
                        failed,
                        cx,
                    );
                }
                WorkspaceEvent::PluginSettings { plugin, settings } => {
                    let saved =
                        handler.save_plugin_settings(plugin, settings.clone());
                    let catalog = handler.catalog();
                    workspace.update(cx, |ws, cx| {
                        ws.apply(HostUpdate::catalog(catalog), cx);
                        if let Err(error) = saved {
                            ws.apply(HostUpdate::alert(format!("Could not save {plugin}'s settings"), format!("{error:#}")), cx);
                        }
                    });
                }
                WorkspaceEvent::PluginRecord { run, plugin, body } => {
                    if let Err(error) =
                        handler.store_plugin_record(run, plugin, body)
                    {
                        // The interface showed the change already: put
                        // back what the plugin will read, and say so.
                        let records = handler.plugin_records(run, plugin);
                        workspace.update(cx, |ws, cx| {
                            ws.apply(HostUpdate::PluginRestate { run: run.clone(), plugin: plugin.clone(), records }, cx);
                            ws.apply(HostUpdate::alert(format!("Could not save what {plugin} changed"), format!("{error:#}")), cx);
                        });
                    }
                }
                WorkspaceEvent::CloseRun { run } => {
                    if let Err(error) = handler.set_closed(run, true) {
                        eprintln!("tau-ui: cannot save closed runs: {error:#}");
                    }
                }
                WorkspaceEvent::Resume { run, prompt, model } => {
                    match handler.resume(run, prompt, model) {
                        Ok(()) => {
                            // A message opens a closed conversation
                            // again.
                            let _ = handler.set_closed(run, false);
                            let starting = handler.starting_of(run, model);
                            workspace.update(cx, |ws, cx| {
                                for (plugin, body) in starting {
                                    ws.apply(HostUpdate::PluginFold { run: run.clone(), plugin, body }, cx);
                                }
                            });
                        }
                        Err(error) => workspace.update(cx, |ws, cx| {
                            ws.apply(HostUpdate::ResumeFailed(run.clone()), cx);
                            ws.apply(HostUpdate::alert("Could not go on with the run", format!("{error:#}")), cx)
                        }),
                    }
                }
                WorkspaceEvent::ResumeCutOff { run } => tau_turn(
                    &handler,
                    run,
                    CUT_OFF.to_owned(),
                    |host, run, _| host.resume_cut_off(run),
                    "Could not resume the run",
                    &workspace,
                    cx,
                ),
                WorkspaceEvent::Fork {
                    run,
                    turn,
                    prompt,
                    model,
                } => match handler.fork(run, *turn, prompt, model) {
                    Ok(view) => {
                        let run = view.id.clone();
                        workspace.update(cx, |ws, cx| ws.apply(HostUpdate::Run(Box::new(view)), cx));
                        title_in_background(&handler, &run, prompt, &workspace, cx);
                    }
                    Err(error) => workspace.update(cx, |ws, cx| {
                        ws.apply(HostUpdate::alert("Could not fork the run", format!("{error:#}")), cx)
                    }),
                },
                WorkspaceEvent::CompareCode { main, fork } => {
                    let code = |ws: &mut Workspace, main, fork, code, cx: &mut Context<Workspace>| {
                        ws.apply(HostUpdate::BranchCode { main, fork, code }, cx)
                    };
                    let (job_main, job_fork) = (main.clone(), fork.clone());
                    let (main, fork) = (main.clone(), fork.clone());
                    let (failed_main, failed_fork) = (main.clone(), fork.clone());
                    off_thread(
                        &handler,
                        &workspace,
                        move |host| {
                            host.block_on(host.branch_code(&job_main, &job_fork))
                        },
                        move |ws, ready, cx| code(ws, main, fork, CodeState::Ready(ready), cx),
                        move |ws, error, cx| {
                            code(ws, failed_main, failed_fork, CodeState::Unavailable(error), cx)
                        },
                        cx,
                    );
                }
                WorkspaceEvent::SaveModelSettings(settings) => {
                    if let Err(error) = handler.save_settings(settings.clone())
                    {
                        workspace.update(cx, |ws, cx| {
                            ws.apply(HostUpdate::alert("Could not save the model settings", format!("{error:#}")), cx)
                        });
                    }
                }
                WorkspaceEvent::PreviewLanding { run } => {
                    let preview = |ws: &mut Workspace, run, preview, cx: &mut Context<Workspace>| {
                        ws.apply(HostUpdate::LandingPreview { run, preview }, cx)
                    };
                    let (job_run, run, failed_run) = (run.clone(), run.clone(), run.clone());
                    off_thread(
                        &handler,
                        &workspace,
                        move |host| host.preview_landing(&job_run),
                        move |ws, landing, cx| preview(ws, run, Ok(landing), cx),
                        move |ws, error, cx| preview(ws, failed_run, Err(error), cx),
                        cx,
                    );
                }
                // A chat lands by joining its main chat's queue, which
                // lands it at once when main is idle and nothing waits
                // before it (ADR 0021). What conflicts, main resolves,
                // in a turn tau starts (ADR 0014).
                WorkspaceEvent::Land { run } => {
                    let (job_run, failed_run) = (run.clone(), run.clone());
                    let shower = handler.clone();
                    off_thread(
                        &handler,
                        &workspace,
                        move |host| host.queue_landing(&job_run),
                        move |ws, report, cx| {
                            let workspace = cx.entity();
                            let host = shower.clone();
                            let _ = ws;
                            cx.defer(move |cx| show_drain(&host, report, &workspace, cx));
                        },
                        move |ws, error, cx| {
                            ws.apply(HostUpdate::Landed { run: failed_run, landing: Err(error) }, cx)
                        },
                        cx,
                    );
                }
                WorkspaceEvent::Unqueue { run } => {
                    let run = run.clone();
                    drain_off_thread(
                        &handler,
                        &workspace,
                        move |host| host.unqueue(&run),
                        "Could not take the chat out of the queue",
                        cx,
                    );
                }
                WorkspaceEvent::DismissConflicts { main } => {
                    let main = main.clone();
                    drain_off_thread(
                        &handler,
                        &workspace,
                        move |host| host.dismiss_conflicts(&main),
                        "Could not save that",
                        cx,
                    );
                }
                WorkspaceEvent::ResolveAgain { main } => {
                    match handler.resolve_again_prompt(main) {
                        Ok(prompt) => resolve_main(&handler, main, prompt, &workspace, cx),
                        Err(error) => workspace.update(cx, |ws, cx| {
                            ws.apply(HostUpdate::alert("Could not resolve again", format!("{error:#}")), cx)
                        }),
                    }
                }
                WorkspaceEvent::DropChild { run } => {
                    // A dropped chat leaves the queue it waited in.
                    let (job_run, run) = (run.clone(), run.clone());
                    let failed_run = run.clone();
                    let shower = handler.clone();
                    off_thread(
                        &handler,
                        &workspace,
                        move |host| {
                            host.drop_child(&job_run)?;
                            host.unqueue(&job_run)
                        },
                        move |ws, report, cx| {
                            ws.apply(HostUpdate::Dropped { run, result: Ok(()) }, cx);
                            let workspace = cx.entity();
                            let host = shower.clone();
                            cx.defer(move |cx| show_drain(&host, report, &workspace, cx));
                        },
                        move |ws, error, cx| {
                            ws.apply(HostUpdate::Dropped { run: failed_run, result: Err(error) }, cx)
                        },
                        cx,
                    );
                }
                WorkspaceEvent::KeepBranch { run } => {
                    if let Err(error) = handler.keep_branch(run) {
                        eprintln!(
                            "tau-ui: cannot drop the other branches: {error:#}"
                        );
                    }
                }
                WorkspaceEvent::HideRepo { repo } => {
                    if let Err(error) = handler.hide_repo(repo) {
                        eprintln!(
                            "tau-ui: cannot save the repository list: {error:#}"
                        );
                    }
                }
                WorkspaceEvent::OpenRepos(open) => {
                    if let Err(error) = handler.set_open_repos(open.clone()) {
                        eprintln!(
                            "tau-ui: cannot save the repository list: {error:#}"
                        );
                    }
                }
                // `phone_server::serve` handles these in its own
                // subscription.
                WorkspaceEvent::Phones(_) => {}
                WorkspaceEvent::Steer { run, text } => {
                    if let Err(error) = handler.steer(run, text) {
                        workspace.update(cx, |ws, cx| {
                            ws.apply(HostUpdate::alert("Could not steer the run", format!("{error:#}")), cx)
                        });
                    }
                }
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
                    let catalog = host.catalog();
                    let _ = workspace.update(cx, |ws, cx| {
                        ws.apply(HostUpdate::catalog(catalog), cx)
                    });
                }
                // A pull request that keeps pushing takes the commits the
                // turn made.
                if let RunEvent::TurnEnd { run, .. } = &event
                    && host.keeps_pushing(run)
                {
                    let (pusher, run) = (host.clone(), run.clone());
                    host.runtime.spawn_blocking(move || {
                        if let Err(error) = pusher.push_later_commits(&run) {
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
                if let RunEvent::RunEnd { run, .. } = &event {
                    host.ending
                        .lock()
                        .expect("not poisoned")
                        .insert(run.clone());
                }
                // A main chat's turn ended: what it left in conflict
                // marks it, and what waited lands (ADR 0021).
                let main_ended = match &event {
                    RunEvent::RunEnd { run, .. } if host.is_main(run) => {
                        Some(run.clone())
                    }
                    _ => None,
                };
                let applied = workspace.update(cx, |ws, cx| {
                    ws.apply(HostUpdate::Event(event.clone()), cx);
                    if let Some(refusal) = &refusal {
                        ws.apply(HostUpdate::PlanRefusal(refusal.clone()), cx);
                    }
                });
                if applied.is_err() {
                    break;
                }
                if let (Some(main), Some(entity)) =
                    (main_ended, workspace.upgrade())
                {
                    let host = host.clone();
                    cx.update(|cx| {
                        drain_off_thread(
                            &host,
                            &entity,
                            move |host| host.main_turn_ended(&main),
                            "Could not land what waits on main",
                            cx,
                        )
                    });
                }
            }
            // Keep the host, and its runtime, alive as long as events flow.
            drop(host);
        })
        .detach();
    }
}
