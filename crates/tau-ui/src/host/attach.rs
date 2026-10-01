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

/// `paths` as the model reads them: `a.rs`, `b.rs` and `c.rs`.
pub(super) fn code_list(paths: &[String]) -> String {
    let quoted: Vec<String> =
        paths.iter().map(|path| format!("`{path}`")).collect();
    match quoted.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// Starts `run`'s resolving turn with `prompt` (ADR 0014): its chat
/// shows the message as tau's, then the host resumes it.
pub(super) fn resolve(
    host: &Arc<Host>,
    run: &RunId,
    prompt: String,
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
    if let Err(error) = host.start_resolving(run, &prompt) {
        workspace.update(cx, |ws, cx| {
            ws.apply(HostUpdate::ResumeFailed(run.clone()), cx);
            ws.apply(
                HostUpdate::alert(
                    "Could not start resolving the conflicts",
                    format!("{error:#}"),
                ),
                cx,
            );
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
        for slot in slots {
            refresh_when_imported(&host, &slot, workspace, cx);
            // What changed while tau was closed comes in, quietly.
            update_in_background(&host, &slot.name, workspace, false, cx);
        }
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
                    // A message opens a closed conversation again.
                    let _ = handler.set_closed(run, false);
                    match handler.resume(run, prompt, model) {
                        Ok(()) => {
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
                    let preview = handler
                        .preview_landing(run)
                        .map_err(|error| format!("{error:#}"));
                    workspace.update(cx, |ws, cx| {
                        ws.apply(HostUpdate::LandingPreview { run: run.clone(), preview }, cx)
                    });
                }
                WorkspaceEvent::Land { run } => {
                    let parent = handler.parent_of(run).ok();
                    let landed =
                        handler.land(run).map_err(|error| format!("{error:#}"));
                    let conflicts = landed
                        .as_ref()
                        .map(|landing| landing.conflicts.clone())
                        .unwrap_or_default();
                    workspace.update(cx, |ws, cx| ws.apply(HostUpdate::Landed { run: run.clone(), landing: landed }, cx));
                    // What conflicts, the parent resolves, in a turn tau
                    // starts (ADR 0014).
                    if let Some(parent) = parent.filter(|_| !conflicts.is_empty()) {
                        let title = handler.title_of(run).unwrap_or_else(|_| run.0.to_string());
                        let prompt = format!(
                            "Landing `{title}` left conflicts in {}. Resolve them, \
                             and commit the resolution.",
                            code_list(&conflicts)
                        );
                        resolve(&handler, &parent, prompt, &workspace, cx);
                    }
                }
                WorkspaceEvent::DropChild { run } => {
                    let dropped = handler
                        .drop_child(run)
                        .map_err(|error| format!("{error:#}"));
                    workspace.update(cx, |ws, cx| ws.apply(HostUpdate::Dropped { run: run.clone(), result: dropped }, cx));
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
                WorkspaceEvent::Steer { run, text } => handler.steer(run, text),
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
                let applied = workspace.update(cx, |ws, cx| {
                    ws.apply(HostUpdate::Event(event.clone()), cx);
                    if let Some(refusal) = &refusal {
                        ws.apply(HostUpdate::PlanRefusal(refusal.clone()), cx);
                    }
                });
                if applied.is_err() {
                    break;
                }
            }
            // Keep the host, and its runtime, alive as long as events flow.
            drop(host);
        })
        .detach();
    }
}
