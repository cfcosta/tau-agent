//! tau-mcp and tau-codemode against real MCP servers
//! (`docs/reference/mcp.md`, "Against real servers").
//!
//! ```sh
//! nix shell nixpkgs#nodejs nixpkgs#uv nixpkgs#git -c \
//!   cargo run -p tau-mcp --example live -- [--model gpt-5.5]
//! ```
//!
//! It writes a temporary `mcp.json` (never the user's) naming four
//! servers, found on `PATH`:
//!
//! - `everything`: `npx -y @modelcontextprotocol/server-everything stdio`;
//! - `everything_http`: the same server in its streamable HTTP mode, which
//!   this example starts on a free port;
//! - `git`: `uvx mcp-server-git` over a temporary repository, as a
//!   `codemode` server;
//! - `fetch`: `uvx mcp-server-fetch`, as a `codemode` server, fetching a
//!   page from a local HTTP server.
//!
//! It then checks, printing `ok` or `FAIL` per check: listing tools,
//! resources, templates and prompts; calls with text, structured
//! content, images, errors and progress; cancellation; reading
//! resources; getting prompts; `list_changed`; roots; Codemode scripts
//! calling the servers in parallel, `search_tools` and
//! `describe_namespace` on the real catalog, and every tool's Luau
//! signature parsing; and that closing the plugin leaves no server
//! process behind. With `--model`, it also runs the agent on the signed-in
//! ChatGPT account (tau's store, `~/.config/tau/chatgpt`), asking the
//! model for a direct MCP call and a codemode script, at most 6 turns.
//!
//! The exit status is the number of failed checks.

use std::{
    collections::HashSet,
    path::PathBuf,
    process::Stdio,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use futures_util::StreamExt;
use mlua::Lua;
use serde_json::{Value, json};
use tau_agent::{agent::Agent, event::RunEvent, limits::Limits};
use tau_ai::message::InputBlock;
use tau_codemode::{
    Codemode,
    ToolEntry,
    signature::{CALL_TOOL_RESULT_TYPES, render_tool},
};
use tau_mcp::{McpPlugin, connection::State};
use tau_store::Store;
use tau_testing::scripted::ScriptedModel;
use tokio::{io::AsyncWriteExt, net::TcpListener};
use tokio_util::sync::CancellationToken;

type Error = Box<dyn std::error::Error>;

/// The servers' checks, counted.
#[derive(Default)]
struct Report {
    passed: usize,
    failed: Vec<String>,
}

impl Report {
    fn check(&mut self, name: &str, result: Result<String, String>) {
        match result {
            Ok(note) => {
                self.passed += 1;
                if note.is_empty() {
                    println!("ok   {name}");
                } else {
                    println!("ok   {name}: {note}");
                }
            }
            Err(why) => {
                println!("FAIL {name}: {why}");
                self.failed.push(name.to_owned());
            }
        }
    }
}

fn ensure(cond: bool, why: impl FnOnce() -> String) -> Result<(), String> {
    if cond { Ok(()) } else { Err(why()) }
}

/// The absolute path of `program` on `PATH`.
fn which(program: &str) -> Result<PathBuf, Error> {
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .map(|dir| dir.join(program))
        .find(|path| path.is_file())
        .ok_or_else(|| {
            format!(
                "`{program}` is not on PATH: run under `nix shell \
                 nixpkgs#nodejs nixpkgs#uv nixpkgs#git -c ...`"
            )
            .into()
        })
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .map(|addr| addr.port())
        .expect("a free port")
}

/// Serves one fixed page on a local port, for the fetch server.
async fn page_server() -> (u16, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut buffer = [0u8; 4096];
                let _ = tokio::io::AsyncReadExt::read(&mut socket, &mut buffer)
                    .await;
                let request = String::from_utf8_lossy(&buffer);
                let (status, body) = if request.starts_with("GET /robots.txt") {
                    ("404 Not Found", String::new())
                } else {
                    (
                        "200 OK",
                        "<html><head><title>tau live</title></head><body>\
                         <h1>Hello from tau</h1><p>The marker is \
                         kumquat-42.</p></body></html>"
                            .to_owned(),
                    )
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: text/html\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
            });
        }
    });
    (port, task)
}

/// The processes descended from this one, by `/proc`.
fn descendants() -> HashSet<u32> {
    let mut parents: Vec<(u32, u32)> = Vec::new();
    if let Ok(entries) = std::fs::read_dir("/proc") {
        for entry in entries.flatten() {
            let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>()
            else {
                continue;
            };
            let Ok(stat) = std::fs::read_to_string(entry.path().join("stat"))
            else {
                continue;
            };
            // The fields after the command, which may hold spaces.
            let Some((_, rest)) = stat.rsplit_once(')') else {
                continue;
            };
            let fields: Vec<&str> = rest.split_whitespace().collect();
            // State is fields[0]; zombies are gone for our purpose.
            if fields.first() == Some(&"Z") {
                continue;
            }
            if let Some(ppid) = fields.get(1).and_then(|p| p.parse().ok()) {
                parents.push((pid, ppid));
            }
        }
    }
    let mut found: HashSet<u32> = HashSet::new();
    let mut frontier = vec![std::process::id()];
    while let Some(parent) = frontier.pop() {
        for &(pid, ppid) in &parents {
            if ppid == parent && found.insert(pid) {
                frontier.push(pid);
            }
        }
    }
    found
}

fn alive(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|stat| {
            stat.rsplit_once(')')
                .map(|(_, rest)| !rest.trim_start().starts_with('Z'))
        })
        .unwrap_or(false)
}

fn text_of(result: &Value) -> String {
    result["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|block| block["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

fn blocks_text(blocks: &[InputBlock]) -> String {
    blocks
        .iter()
        .filter_map(|block| match block {
            InputBlock::Text(text) => Some(text.text.as_str()),
            InputBlock::Image(_) => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Waits up to `limit` for `ready`.
async fn wait_until(limit: Duration, ready: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if ready() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    ready()
}

struct Setup {
    _dir: tempfile::TempDir,
    repo: PathBuf,
    http: tokio::process::Child,
    http_port: u16,
    page_port: u16,
    page: tokio::task::JoinHandle<()>,
}

/// Writes the temporary `mcp.json` and repository, and starts the HTTP
/// server.
async fn setup() -> Result<(Setup, PathBuf), Error> {
    let npx = which("npx")?;
    let uvx = which("uvx")?;
    let git = which("git")?;
    let dir = tempfile::Builder::new().prefix("tau-mcp-live-").tempdir()?;
    let user = dir.path().join("config");
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&user)?;
    std::fs::create_dir_all(&repo)?;
    std::fs::write(repo.join("README.md"), "# live\n")?;
    let run_git = |args: &[&str]| -> Result<(), Error> {
        let status = std::process::Command::new(&git)
            .args(args)
            .current_dir(&repo)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "tau")
            .env("GIT_AUTHOR_EMAIL", "tau@localhost")
            .env("GIT_COMMITTER_NAME", "tau")
            .env("GIT_COMMITTER_EMAIL", "tau@localhost")
            .stdout(Stdio::null())
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("git {args:?} failed").into())
        }
    };
    run_git(&["init", "-q", "-b", "main"])?;
    run_git(&["add", "README.md"])?;
    run_git(&["commit", "-q", "-m", "first commit"])?;
    std::fs::write(repo.join("new.txt"), "untracked\n")?;

    let http_port = free_port();
    let http = tokio::process::Command::new(&npx)
        .args([
            "-y",
            "@modelcontextprotocol/server-everything",
            "streamableHttp",
        ])
        .env("PORT", http_port.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .kill_on_drop(true)
        .spawn()?;
    let (page_port, page) = page_server().await;

    let path = std::env::var("PATH").unwrap_or_default();
    let config = json!({
        "mcpServers": {
            "everything": {
                "command": npx,
                "args": ["-y", "@modelcontextprotocol/server-everything", "stdio"],
                "description": "The MCP reference server that exercises every feature."
            },
            "everything_http": {
                "url": format!("http://127.0.0.1:{http_port}/mcp"),
                "exposure": "codemode"
            },
            "git": {
                "command": uvx,
                "args": ["mcp-server-git", "--repository", repo],
                // mcp-server-git runs the git binary.
                "env": { "PATH": path },
                "exposure": "codemode"
            },
            "fetch": {
                "command": uvx,
                "args": ["mcp-server-fetch", "--ignore-robots-txt"],
                "exposure": "codemode"
            }
        }
    });
    std::fs::write(
        user.join("mcp.json"),
        serde_json::to_string_pretty(&config)?,
    )?;
    Ok((
        Setup {
            _dir: dir,
            repo,
            http,
            http_port,
            page_port,
            page,
        },
        user,
    ))
}

/// Waits for the HTTP server to listen.
async fn http_ready(port: u16) -> bool {
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    false
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<(), Error> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let model = args
        .iter()
        .position(|arg| arg == "--model")
        .and_then(|at| args.get(at + 1).cloned());
    let (mut setup, user) = setup().await?;
    let mut report = Report::default();

    report.check(
        "the HTTP server listens",
        if http_ready(setup.http_port).await {
            Ok(format!("port {}", setup.http_port))
        } else {
            Err("nothing listens after 60 s".into())
        },
    );
    let before = descendants();

    let mcp = McpPlugin::builder()
        .user_dir(&user)
        .repo(&setup.repo)
        .build();
    let started = Instant::now();
    for connection in mcp.connections() {
        let cancel = CancellationToken::new();
        let wait = tokio::time::timeout(
            Duration::from_secs(90),
            connection.settled(&cancel),
        );
        let _ = wait.await;
        let status = connection.status();
        report.check(
            &format!("{} connects", connection.name()),
            if status.state == State::Connected {
                Ok(format!(
                    "protocol {}, {} tools, {} resources, {} templates, {} prompts after {:.1} s",
                    connection.protocol().unwrap_or_default(),
                    connection.tools().len(),
                    connection.resources().len(),
                    connection.templates().len(),
                    connection.prompts().len(),
                    started.elapsed().as_secs_f64()
                ))
            } else {
                Err(format!("{}: {:?}", status.state, status.error))
            },
        );
    }
    report.check("no configuration errors", {
        let errors = mcp.config_errors();
        ensure(errors.is_empty(), || format!("{errors:?}"))
            .map(|()| String::new())
    });

    servers(&mcp, &setup, &mut report).await;
    results(&mcp, &mut report).await;
    codemode(&mcp, &setup, &mut report).await;
    resilience(&mcp, &mut setup, &mut report).await;
    if let Some(model) = &model {
        agent(&mcp, model, &setup.repo, &mut report).await;
    }

    // Closing leaves no server process behind.
    // The HTTP server is this example's own, not the plugin's.
    let spawned: HashSet<u32> = descendants()
        .difference(&before)
        .copied()
        .filter(|pid| {
            !std::fs::read_to_string(format!("/proc/{pid}/cmdline"))
                .unwrap_or_default()
                .contains("streamableHttp")
        })
        .collect();
    mcp.shutdown().await;
    drop(mcp);
    let gone = wait_until(Duration::from_secs(5), || {
        spawned.iter().all(|pid| !alive(*pid))
    })
    .await;
    report.check(
        "closing ends every stdio server process",
        if spawned.is_empty() {
            Err("no server processes were seen".into())
        } else if gone {
            Ok(format!("{} processes ended", spawned.len()))
        } else {
            let left: Vec<String> = spawned
                .iter()
                .filter(|pid| alive(**pid))
                .map(|pid| {
                    let cmd =
                        std::fs::read_to_string(format!("/proc/{pid}/cmdline"))
                            .unwrap_or_default()
                            .replace('\0', " ");
                    format!("{pid} {cmd}")
                })
                .collect();
            Err(format!("still running: {left:?}"))
        },
    );

    // The HTTP server is ours: end its group.
    if let Some(pid) = setup.http.id() {
        // SAFETY: signals the process group this example started.
        unsafe { libc::kill(-(pid as i32), libc::SIGTERM) };
    }
    let _ =
        tokio::time::timeout(Duration::from_secs(5), setup.http.wait()).await;
    let _ = setup.http.kill().await;
    setup.page.abort();

    println!(
        "\n{} passed, {} failed{}",
        report.passed,
        report.failed.len(),
        if report.failed.is_empty() {
            String::new()
        } else {
            format!(": {}", report.failed.join(", "))
        }
    );
    std::process::exit(report.failed.len().min(255) as i32);
}

/// The servers through tau-mcp's connections, without a model.
async fn servers(mcp: &McpPlugin, setup: &Setup, report: &mut Report) {
    let cancel = CancellationToken::new();
    let find = |name: &str| {
        mcp.connections()
            .iter()
            .find(|connection| connection.name() == name)
            .cloned()
            .expect("a configured server")
    };
    let none = |_: tau_mcp::connection::Progress| {};

    for name in ["everything", "everything_http"] {
        let server = find(name);
        if server.status().state != State::Connected {
            continue;
        }
        let tools: Vec<String> =
            server.tools().into_iter().map(|tool| tool.name).collect();
        report.check(
            &format!("{name}: lists its tools"),
            ensure(
                [
                    "echo",
                    "get-sum",
                    "get-structured-content",
                    "get-tiny-image",
                ]
                .iter()
                .all(|tool| tools.iter().any(|t| t == tool)),
                || format!("{tools:?}"),
            )
            .map(|()| tools.join(", ")),
        );
        // Tools the server registers after it initializes, once it knows
        // the client has roots, come through list_changed.
        let late = wait_until(Duration::from_secs(10), || {
            server
                .tools()
                .iter()
                .any(|tool| tool.name == "get-roots-list")
        })
        .await;
        report.check(
            &format!("{name}: tools/list_changed brings get-roots-list"),
            ensure(late, || "get-roots-list never listed".into())
                .map(|()| String::new()),
        );

        let echo = server
            .call("echo", json!({"message": "hi from tau"}), &none, &cancel)
            .await;
        report.check(
            &format!("{name}: echo"),
            echo.map_err(|e| e.to_string()).and_then(|result| {
                let text = text_of(&result);
                ensure(text.contains("hi from tau"), || text.clone())
                    .map(|()| text)
            }),
        );
        let structured = server
            .call(
                "get-structured-content",
                json!({"location": "Chicago"}),
                &none,
                &cancel,
            )
            .await;
        report.check(
            &format!("{name}: structured content"),
            structured.map_err(|e| e.to_string()).and_then(|result| {
                let content = &result["structuredContent"];
                ensure(content["temperature"].is_number(), || {
                    result.to_string()
                })
                .map(|()| content.to_string())
            }),
        );
        let image = server
            .call("get-tiny-image", json!({}), &none, &cancel)
            .await;
        report.check(
            &format!("{name}: image content"),
            image.map_err(|e| e.to_string()).and_then(|result| {
                let types: Vec<&str> = result["content"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|block| block["type"].as_str())
                    .collect();
                ensure(types.contains(&"image"), || format!("{types:?}"))
                    .map(|()| format!("{types:?}"))
            }),
        );
        let bad = server
            .call("get-sum", json!({"a": "one", "b": 2}), &none, &cancel)
            .await;
        report.check(
            &format!("{name}: bad arguments come back as an error"),
            match bad {
                Ok(result) if result["isError"] == true => Ok(text_of(&result)),
                Ok(result) => Err(format!("not an error: {result}")),
                Err(failure) => Ok(format!("protocol error: {failure}")),
            },
        );
        let updates = Arc::new(Mutex::new(Vec::new()));
        let seen = updates.clone();
        let progress = move |p: tau_mcp::connection::Progress| {
            seen.lock().unwrap().push((p.progress, p.total));
        };
        let long = server
            .call(
                "trigger-long-running-operation",
                json!({"duration": 1, "steps": 4}),
                &progress,
                &cancel,
            )
            .await;
        let updates = updates.lock().unwrap().clone();
        report.check(
            &format!("{name}: progress"),
            long.map_err(|e| e.to_string()).and_then(|result| {
                ensure(updates.len() == 4, || {
                    format!("{updates:?} for {}", text_of(&result))
                })
                .map(|()| format!("{updates:?}"))
            }),
        );
        let stop = CancellationToken::new();
        let stopper = stop.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(500)).await;
            stopper.cancel();
        });
        let at = Instant::now();
        let cancelled = server
            .call(
                "trigger-long-running-operation",
                json!({"duration": 20, "steps": 2}),
                &none,
                &stop,
            )
            .await;
        report.check(
            &format!("{name}: cancel ends the call at once"),
            match cancelled {
                Err(tau_mcp::connection::CallFailure::Cancelled { .. })
                    if at.elapsed() < Duration::from_secs(3) =>
                {
                    Ok(format!("after {:.1} s", at.elapsed().as_secs_f64()))
                }
                other => Err(format!("{other:?} after {:?}", at.elapsed())),
            },
        );
        let after = server
            .call("get-sum", json!({"a": 2, "b": 40}), &none, &cancel)
            .await;
        report.check(
            &format!("{name}: the connection serves calls after a cancel"),
            after.map_err(|e| e.to_string()).and_then(|result| {
                let text = text_of(&result);
                ensure(text.contains("42"), || text.clone()).map(|()| text)
            }),
        );

        // Resources.
        let resources = server.resources();
        let templates = server.templates();
        report.check(
            &format!("{name}: lists resources and templates"),
            ensure(!resources.is_empty() && !templates.is_empty(), || {
                format!(
                    "{} resources, {} templates",
                    resources.len(),
                    templates.len()
                )
            })
            .map(|()| {
                format!(
                    "{} resources, templates {:?}",
                    resources.len(),
                    templates
                        .iter()
                        .map(|t| &t.uri_template)
                        .collect::<Vec<_>>()
                )
            }),
        );
        if let Some(first) = resources.first() {
            let read = server.read_resource(&first.uri, &cancel).await;
            report.check(
                &format!("{name}: reads {}", first.uri),
                read.and_then(|result| {
                    ensure(
                        result["contents"]
                            .as_array()
                            .is_some_and(|c| !c.is_empty()),
                        || result.to_string(),
                    )
                    .map(|()| String::new())
                }),
            );
        }
        for uri in [
            "demo://resource/dynamic/text/3",
            "demo://resource/dynamic/blob/3",
        ] {
            let read = server.read_resource(uri, &cancel).await;
            report.check(
                &format!("{name}: reads the template's {uri}"),
                read.and_then(|result| {
                    let first = &result["contents"][0];
                    ensure(
                        first["text"].is_string() || first["blob"].is_string(),
                        || result.to_string(),
                    )
                    .map(|()| first["mimeType"].to_string())
                }),
            );
        }
        // A session resource the server registers mid-session comes
        // through resources/list_changed.
        let gzip = server
            .call(
                "gzip-file-as-resource",
                json!({
                    "name": "tau.txt.gz",
                    "data": "data:text/plain;base64,aGVsbG8gdGF1",
                }),
                &none,
                &cancel,
            )
            .await;
        let listed = wait_until(Duration::from_secs(5), || {
            server
                .resources()
                .iter()
                .any(|r| r.uri.ends_with("/session/tau.txt.gz"))
        })
        .await;
        report.check(
            &format!("{name}: resources/list_changed lists a new resource"),
            match gzip {
                Err(error) => Err(error.to_string()),
                Ok(result) if result["isError"] == true => {
                    Err(text_of(&result))
                }
                Ok(_) => ensure(listed, || {
                    format!(
                        "{:?}",
                        server
                            .resources()
                            .iter()
                            .map(|r| &r.uri)
                            .collect::<Vec<_>>()
                    )
                })
                .map(|()| String::new()),
            },
        );

        // Roots: the repository, for a server with a repository.
        let roots = server
            .call("get-roots-list", json!({}), &none, &cancel)
            .await;
        report.check(
            &format!("{name}: the server sees the repository as its root"),
            roots.map_err(|e| e.to_string()).and_then(|result| {
                let text = text_of(&result);
                ensure(text.contains(&setup.repo.display().to_string()), || {
                    text.clone()
                })
                .map(|()| String::new())
            }),
        );
    }

    // Prompts through the plugin, as the composer gets them.
    let commands: Vec<String> =
        mcp.prompts().into_iter().map(|p| p.command).collect();
    report.check(
        "prompts are listed as commands",
        ensure(
            commands.iter().any(|c| c == "mcp__everything__args_prompt"),
            || format!("{commands:?}"),
        )
        .map(|()| commands.join(", ")),
    );
    for (command, arguments, expect) in [
        ("mcp__everything__simple_prompt", "", "simple"),
        (
            "mcp__everything__args_prompt",
            "city=Lisbon state='Lisboa district'",
            "Lisbon",
        ),
        (
            "mcp__everything__resource_prompt",
            "resourceType=Text resourceId=2",
            "Resource 2: This is a plaintext resource",
        ),
        (
            "mcp__fetch__fetch",
            &format!("url=http://127.0.0.1:{}/", setup.page_port),
            "kumquat-42",
        ),
    ] {
        let got = mcp.get_prompt(command, arguments, &cancel).await;
        report.check(
            &format!("prompt {command} {arguments}"),
            got.and_then(|text| {
                ensure(text.contains(expect), || text.clone()).map(|()| {
                    text.lines().next().unwrap_or_default().to_owned()
                })
            }),
        );
    }
    let missing = mcp
        .get_prompt("mcp__everything__args_prompt", "state=x", &cancel)
        .await;
    report.check(
        "a prompt missing a required argument fails with its usage",
        match missing {
            Err(error) if error.contains("Usage") => Ok(error),
            other => Err(format!("{other:?}")),
        },
    );

    // The Python servers.
    let git = find("git");
    if git.status().state == State::Connected {
        let status = git
            .call(
                "git_status",
                json!({"repo_path": setup.repo}),
                &none,
                &cancel,
            )
            .await;
        report.check(
            "git: git_status",
            status.map_err(|e| e.to_string()).and_then(|result| {
                let text = text_of(&result);
                ensure(text.contains("new.txt"), || text.clone()).map(|()| {
                    text.lines().next().unwrap_or_default().to_owned()
                })
            }),
        );
        let bad = git
            .call(
                "git_log",
                json!({"repo_path": "/nonexistent/repo"}),
                &none,
                &cancel,
            )
            .await;
        report.check(
            "git: a failing call is an error result",
            match bad {
                Ok(result) if result["isError"] == true => Ok(text_of(&result)),
                Ok(result) => Err(format!("not an error: {result}")),
                Err(failure) => Ok(format!("protocol error: {failure}")),
            },
        );
    }
    let fetch = find("fetch");
    if fetch.status().state == State::Connected {
        let page = fetch
            .call(
                "fetch",
                json!({"url": format!("http://127.0.0.1:{}/", setup.page_port)}),
                &none,
                &cancel,
            )
            .await;
        report.check(
            "fetch: fetches a local page",
            page.map_err(|e| e.to_string()).and_then(|result| {
                let text = text_of(&result);
                ensure(text.contains("kumquat-42"), || text.clone())
                    .map(|()| String::new())
            }),
        );
    }
}

/// What the model reads of real results, through the loop.
async fn results(mcp: &McpPlugin, report: &mut Report) {
    let calls = [
        ("mcp__everything__get_resource_links", json!({"count": 2})),
        (
            "mcp__everything__get_resource_reference",
            json!({"resourceType": "Blob", "resourceId": 4}),
        ),
        (
            "mcp__everything__get_resource_reference",
            json!({"resourceType": "Text", "resourceId": 5}),
        ),
        (
            "mcp__everything__get_annotated_message",
            json!({"messageType": "success", "includeImage": true}),
        ),
        (
            "mcp__everything__get_structured_content",
            json!({"location": "Los Angeles"}),
        ),
        ("mcp__everything__get_sum", json!({"a": "x", "b": 1})),
        ("list_mcp_resources", json!({"server": "everything"})),
        ("list_mcp_resource_templates", json!({})),
        (
            "read_mcp_resource",
            json!({"server": "everything", "uri": "demo://resource/dynamic/blob/1"}),
        ),
        (
            "read_mcp_resource",
            json!({"server": "everything", "uri": "demo://resource/static/document/features.md"}),
        ),
        ("mcp__everything__toggle_simulated_logging", json!({})),
        ("mcp__everything__toggle_simulated_logging", json!({})),
    ];
    let mut llm = ScriptedModel::new();
    for (name, args) in calls.iter().cloned() {
        llm = llm.turn(move |t| t.tool_call(name, args));
    }
    llm = llm.turn(|t| t.text("done"));
    let store = Store::memory().await.expect("a store");
    let agent = Agent::new(llm).plugin(mcp.clone());
    let mut run = agent.start("go", &store);
    let mut ends = Vec::new();
    while let Some(event) = run.events().next().await {
        if let RunEvent::ToolEnd {
            parent: None,
            output,
            is_error,
            ..
        } = event
        {
            ends.push((output, is_error));
        }
    }
    let _ = run.outcome().await;
    let get = |n: usize| {
        ends.get(n)
            .map(|(output, is_error)| {
                let images = output
                    .content
                    .iter()
                    .filter(|b| matches!(b, InputBlock::Image(_)))
                    .count();
                (
                    blocks_text(&output.content),
                    *is_error,
                    images,
                    output.details.clone(),
                )
            })
            .unwrap_or_default()
    };
    let show = |text: &str| text.chars().take(300).collect::<String>();
    let (text, failed, ..) = get(0);
    report.check(
        "model: resource links name read_mcp_resource",
        ensure(
            !failed
                && text.contains("[Resource demo://resource/dynamic/")
                && text.contains("read_mcp_resource"),
            || text.clone(),
        )
        .map(|()| show(&text)),
    );
    let (text, failed, ..) = get(1);
    report.check(
        "model: an embedded blob is saved to a file",
        ensure(!failed && text.contains("saved to"), || text.clone())
            .map(|()| show(&text)),
    );
    let (text, failed, ..) = get(2);
    report.check(
        "model: an embedded text resource is its text",
        ensure(!failed && text.contains("Resource 5"), || text.clone())
            .map(|()| show(&text)),
    );
    let (text, failed, images, _) = get(3);
    report.check(
        "model: an annotated message with an image",
        ensure(!failed && images == 1, || {
            format!("{text} ({images} images)")
        })
        .map(|()| show(&text)),
    );
    let (text, failed, _, details) = get(4);
    report.check(
        "model: structured content, and the details keep it for the card",
        ensure(
            !failed
                && details.as_ref().is_some_and(|d| {
                    d["structuredContent"]["humidity"].is_number()
                }),
            || format!("{text} {details:?}"),
        )
        .map(|()| format!("{details:?}")),
    );
    let (text, failed, ..) = get(5);
    report.check(
        "model: a server's validation error fails the call",
        ensure(failed, || text.clone()).map(|()| show(&text)),
    );
    let (text, failed, ..) = get(6);
    report.check(
        "model: list_mcp_resources",
        ensure(
            !failed && text.contains("demo://resource/static/document"),
            || text.clone(),
        )
        .map(|()| format!("{} bytes", text.len())),
    );
    let (text, failed, ..) = get(7);
    report.check(
        "model: list_mcp_resource_templates lists every server's",
        ensure(
            !failed
                && text.contains("everything_http")
                && text.contains("{resourceId}"),
            || text.clone(),
        )
        .map(|()| String::new()),
    );
    let (text, failed, ..) = get(8);
    report.check(
        "model: read_mcp_resource of a blob",
        ensure(!failed && !text.is_empty(), || text.clone())
            .map(|()| show(&text)),
    );
    let (text, failed, ..) = get(9);
    report.check(
        "model: read_mcp_resource of a document",
        ensure(!failed && text.contains("Tools"), || text.clone())
            .map(|()| format!("{} bytes", text.len())),
    );
    let (text, failed, ..) = get(11);
    report.check(
        "model: simulated logging on and off",
        ensure(!failed, || text.clone()).map(|()| show(&text)),
    );
}

/// A stdio server that dies and an HTTP server that restarts: the next
/// call connects again.
async fn resilience(mcp: &McpPlugin, setup: &mut Setup, report: &mut Report) {
    let cancel = CancellationToken::new();
    let none = |_: tau_mcp::connection::Progress| {};
    let find = |name: &str| {
        mcp.connections()
            .iter()
            .find(|connection| connection.name() == name)
            .cloned()
            .expect("a configured server")
    };
    let everything = find("everything");
    let stdio: Vec<u32> = descendants()
        .into_iter()
        .filter(|pid| {
            std::fs::read_to_string(format!("/proc/{pid}/cmdline"))
                .unwrap_or_default()
                .contains("server-everything\0stdio")
        })
        .collect();
    for pid in &stdio {
        // SAFETY: a server process this example's plugin started.
        unsafe { libc::kill(*pid as i32, libc::SIGKILL) };
    }
    let dropped = wait_until(Duration::from_secs(5), || {
        everything.status().state == State::Disconnected
    })
    .await;
    let echo = everything
        .call("echo", json!({"message": "again"}), &none, &cancel)
        .await;
    report.check(
        "a killed stdio server is noticed and the next call connects again",
        match echo {
            Ok(result) if text_of(&result).contains("again") => {
                Ok(format!("{} killed, noticed: {dropped}", stdio.len()))
            }
            other => Err(format!("noticed: {dropped}, then {other:?}")),
        },
    );

    // The HTTP server restarts on the same port: its sessions are gone.
    let http = find("everything_http");
    if let Some(pid) = setup.http.id() {
        // SAFETY: the process group this example started.
        unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
    }
    let _ = setup.http.wait().await;
    let npx = which("npx").expect("npx");
    setup.http = tokio::process::Command::new(&npx)
        .args([
            "-y",
            "@modelcontextprotocol/server-everything",
            "streamableHttp",
        ])
        .env("PORT", setup.http_port.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .kill_on_drop(true)
        .spawn()
        .expect("the HTTP server starts again");
    http_ready(setup.http_port).await;
    let mut tries = Vec::new();
    for _ in 0..3 {
        let result = http
            .call("echo", json!({"message": "after restart"}), &none, &cancel)
            .await;
        let ok =
            matches!(&result, Ok(r) if text_of(r).contains("after restart"));
        tries.push(match result {
            Ok(result) => text_of(&result),
            Err(error) => error.to_string(),
        });
        if ok {
            break;
        }
    }
    report.check(
        "after the HTTP server restarts, a call reaches it within two tries",
        ensure(
            tries.len() <= 2
                && tries.last().is_some_and(|t| t.contains("after restart")),
            || format!("{tries:#?}"),
        )
        .map(|()| format!("{tries:?}")),
    );
}

/// The tools as Codemode sees them.
fn entries(mcp: &McpPlugin) -> Vec<ToolEntry> {
    use tau_agent::tool::AgentTool;
    mcp.tools()
        .iter()
        .map(|tool| ToolEntry {
            name: tool.name().to_owned(),
            description: tool.description().to_owned(),
            input_schema: tool.parameters().clone(),
            output_schema: tool.output_schema().cloned(),
            namespace: Some(tau_mcp::names::namespace(tool.server())),
            sequential: false,
        })
        .collect()
}

/// Codemode scripts against the servers, through the loop, with a
/// scripted model.
async fn codemode(mcp: &McpPlugin, setup: &Setup, report: &mut Report) {
    // Every tool's signature parses in Luau.
    let tools = entries(mcp);
    let lua = Lua::new();
    let bad: Vec<String> = tools
        .iter()
        .filter_map(|tool| {
            let signature = render_tool(tool);
            let code = format!(
                "{CALL_TOOL_RESULT_TYPES}\nlocal tools = {{}}\n{signature}\nend"
            );
            lua.load(code)
                .into_function()
                .err()
                .map(|error| format!("{}: {error}\n{signature}", tool.name))
        })
        .collect();
    report.check(
        "every real tool's Luau signature parses",
        ensure(bad.is_empty(), || bad.join("\n\n"))
            .map(|()| format!("{} tools", tools.len())),
    );

    let page = format!("http://127.0.0.1:{}/", setup.page_port);
    let repo = setup.repo.display().to_string();
    let scripts = [
        // Parallel calls across three servers, two of them codemode-only.
        format!(
            r#"
local t0 = os.clock()
local sum, weather, status, page = parallel(
    function() return tools.mcp__everything__get_sum({{ a = 19, b = 23 }}) end,
    function() return tools.mcp__everything_http__get_structured_content({{ location = "New York" }}) end,
    function() return tools.mcp__git__git_status({{ repo_path = "{repo}" }}) end,
    function() return tools.mcp__fetch__fetch({{ url = "{page}" }}) end
)
return {{
    sum = sum.content[1].text,
    humidity = weather.structuredContent.humidity,
    status = string.find(status.content[1].text, "new.txt") ~= nil,
    page = string.find(page.content[1].text, "kumquat") ~= nil,
}}
"#
        ),
        // Discovery on the real catalog.
        r#"
local found = search_tools("git commit log history")
local names = {}
for _, tool in found do table.insert(names, tool.name) end
local ns = describe_namespace("git")
local fetch = describe_namespace("mcp__fetch")
return {
    found = names,
    namespace = ns and ns.name,
    tools = ns and #ns.tools,
    fetch = fetch and fetch.name,
    described = describe_tool("mcp__everything__get_structured_content") ~= nil,
    all = #ALL_TOOLS,
}
"#
        .to_owned(),
        // An isError result is returned, not raised; an image goes to image().
        r#"
local bad = tools.mcp__git__git_log({ repo_path = "/nonexistent/repo" })
local img = tools.mcp__everything__get_tiny_image({})
for _, block in img.content do
    if block.type == "image" then image(block) end
end
return { isError = bad.isError }
"#
        .to_owned(),
    ];
    let mut llm = ScriptedModel::new().turn(|t| {
        t.tool_call("mcp__everything__echo", json!({"message": "direct"}))
    });
    for code in &scripts {
        let code = code.clone();
        llm =
            llm.turn(move |t| t.tool_call("codemode", json!({ "code": code })));
    }
    llm = llm.turn(|t| t.text("done"));
    let store = Store::memory().await.expect("a store");
    let agent = Agent::new(llm.clone())
        .plugin(mcp.clone())
        .plugin(Codemode::new(None));
    let mut run = agent.start("go", &store);
    let mut ends = Vec::new();
    let mut nested = Vec::new();
    while let Some(event) = run.events().next().await {
        match event {
            RunEvent::ToolEnd {
                parent: None,
                output,
                is_error,
                ..
            } => ends.push((output, is_error)),
            RunEvent::ToolStart {
                tool,
                parent: Some(_),
                ..
            } => nested.push(tool.to_string()),
            _ => {}
        }
    }
    let outcome = run.outcome().await;
    report.check(
        "the scripted run ends",
        outcome
            .map(|o| format!("{:?}", o.stop))
            .map_err(|e| e.to_string()),
    );
    let declared: Vec<String> = llm.requests()[0]
        .settings
        .tools
        .iter()
        .map(|tool| tool.name.clone())
        .collect();
    report.check(
        "direct tools are declared; codemode servers' are not",
        ensure(
            declared.iter().any(|n| n == "mcp__everything__echo")
                && !declared.iter().any(|n| n.starts_with("mcp__git__"))
                && !declared
                    .iter()
                    .any(|n| n.starts_with("mcp__everything_http__"))
                && declared.iter().any(|n| n == "codemode"),
            || format!("{declared:?}"),
        )
        .map(|()| format!("{} declared", declared.len())),
    );
    let context = format!("{:?}", llm.requests()[0].transcript);
    report.check(
        "the run's context lists the servers and direct signatures",
        ensure(
            context.contains("<mcp_servers>")
                && context.contains("mcp__git (codemode)")
                && context.contains("function tools.mcp__everything__"),
            || context.chars().take(2000).collect(),
        )
        .map(|()| String::new()),
    );
    let result = |n: usize| {
        ends.get(n)
            .map(|(output, is_error)| (blocks_text(&output.content), *is_error))
            .unwrap_or_default()
    };
    let (direct, failed) = result(0);
    report.check(
        "the model's direct MCP call",
        ensure(!failed && direct.contains("direct"), || direct.clone())
            .map(|()| direct.clone()),
    );
    let (parallel, failed) = result(1);
    report.check(
        "a script calls four tools on three servers in parallel",
        ensure(
            !failed
                && parallel.contains("42")
                && parallel.contains("\"humidity\":")
                && parallel.contains("\"page\":true")
                && parallel.contains("\"status\":true"),
            || parallel.clone(),
        )
        .map(|()| parallel.lines().last().unwrap_or_default().to_owned()),
    );
    let (discovery, failed) = result(2);
    report.check(
        "search_tools and describe_namespace on the real catalog",
        ensure(
            !failed
                && discovery.contains("mcp__git__git_log")
                && discovery.contains("\"namespace\":\"mcp__git\"")
                && discovery.contains("\"fetch\":\"mcp__fetch\"")
                && discovery.contains("\"described\":true"),
            || discovery.clone(),
        )
        .map(|()| discovery.lines().last().unwrap_or_default().to_owned()),
    );
    let (errors, failed) = result(3);
    let images = ends
        .get(3)
        .map(|(output, _)| {
            output
                .content
                .iter()
                .filter(|b| matches!(b, InputBlock::Image(_)))
                .count()
        })
        .unwrap_or(0);
    report.check(
        "a script gets isError back and passes an image on",
        ensure(
            !failed && errors.contains("\"isError\":true") && images == 1,
            || format!("{errors} ({images} images)"),
        )
        .map(|()| String::new()),
    );
    report.check(
        "nested calls are the scripts' tool calls",
        ensure(nested.len() == 6, || format!("{nested:?}"))
            .map(|()| nested.join(", ")),
    );
}

/// A real model run: the model must call an MCP tool directly and run a
/// codemode script that calls one.
async fn agent(
    mcp: &McpPlugin,
    model: &str,
    repo: &std::path::Path,
    report: &mut Report,
) {
    use tau_ai::{
        chatgpt::{ChatGpt, Store as Accounts},
        client::OpenAi,
    };
    let chatgpt = match Accounts::open_default() {
        Ok(store) => ChatGpt::new(store),
        Err(error) => {
            report.check("the ChatGPT store opens", Err(error.to_string()));
            return;
        }
    };
    let account = match chatgpt.active() {
        Ok(Some(account)) => account,
        other => {
            report.check(
                "a ChatGPT account is signed in",
                Err(format!("{other:?}")),
            );
            return;
        }
    };
    let llm = OpenAi::chatgpt(chatgpt, account);
    let store = Store::memory().await.expect("a store");
    let agent = Agent::new(llm)
        .model(model)
        .instructions(
            "You are testing tool plumbing. Follow the user's steps exactly, \
             using the tools named. Be brief.",
        )
        .limits(Limits::default().max_turns(6))
        .plugin(mcp.clone())
        .plugin(Codemode::new(None));
    let prompt = format!(
        "Step 1: call the tool mcp__everything__get_sum with a=1234 and \
         b=4321, directly (not from a script). Step 2: call the codemode \
         tool once, with a Luau script that runs \
         tools.mcp__git__git_log({{ repo_path = \"{}\", max_count = 1 }}) \
         and tools.mcp__everything__echo({{ message = \"pong\" }}) inside \
         parallel(...) and returns both results' first text. Step 3: reply \
         with the sum and the commit message the log showed.",
        repo.display()
    );
    let started = Instant::now();
    let mut run = agent.start(prompt.as_str(), &store);
    let mut direct = false;
    let mut script = false;
    let mut nested = Vec::new();
    let mut calls = Vec::new();
    while let Some(event) = run.events().next().await {
        if matches!(
            event,
            RunEvent::Retry { .. } | RunEvent::PluginError { .. }
        ) {
            let line = format!("{event:?}");
            println!(
                "     event: {}",
                line.chars().take(400).collect::<String>()
            );
        }
        match event {
            RunEvent::ToolStart {
                tool, parent, args, ..
            } => {
                calls.push(format!(
                    "{}{tool}",
                    if parent.is_some() { "  └ " } else { "" }
                ));
                match (&*tool, parent) {
                    ("mcp__everything__get_sum", None) => direct = true,
                    ("codemode", None) => {
                        script = true;
                        println!(
                            "     script:\n{}",
                            args["code"].as_str().unwrap_or_default()
                        );
                    }
                    (name, Some(_)) => nested.push(name.to_owned()),
                    _ => {}
                }
            }
            RunEvent::ToolEnd {
                is_error: true,
                output,
                parent,
                ..
            } => {
                calls.push(format!(
                    "{}error: {}",
                    if parent.is_some() { "  └ " } else { "" },
                    blocks_text(&output.content)
                        .lines()
                        .last()
                        .unwrap_or_default()
                ));
            }
            _ => {}
        }
    }
    let outcome = run.outcome().await;
    println!("     calls: {calls:#?}");
    match outcome {
        Ok(outcome) => {
            println!("     stop: {:?}; answer: {}", outcome.stop, outcome.text);
            println!(
                "     usage: {:?}, {:.1} s",
                outcome.usage,
                started.elapsed().as_secs_f64()
            );
            report.check(
                &format!("{model}: calls an MCP tool directly"),
                ensure(direct, || format!("{calls:?}")).map(|()| String::new()),
            );
            report.check(
                &format!(
                    "{model}: runs a codemode script that calls MCP tools"
                ),
                ensure(
                    script && nested.iter().any(|n| n.starts_with("mcp__")),
                    || format!("{calls:?}"),
                )
                .map(|()| nested.join(", ")),
            );
            report.check(
                &format!("{model}: answers with the sum"),
                ensure(
                    outcome.text.contains("5555")
                        || outcome.text.contains("5,555"),
                    || outcome.text.clone(),
                )
                .map(|()| String::new()),
            );
        }
        Err(error) => {
            report.check(&format!("{model}: the run"), Err(error.to_string()))
        }
    }
}
