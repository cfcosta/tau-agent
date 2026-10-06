# tau-ui-remote

tau's interface in GPUI: the `Workspace` with its run list, transcript and
inspector, a one-column phone layout below 720 px, every screen, and every
plugin's views. It draws runs and starts none. On the computer, `tau-ui`'s
host drives it; on a phone, its `remote` module does, by pairing with the
tau on a computer. Both feed it the same updates, so the two interfaces
take one path.

## What it provides

| Item                             | What it is                                                                                   |
| -------------------------------- | -------------------------------------------------------------------------------------------- |
| `Workspace`                      | The window's root view: the runs, the screen on show, and the layout for the window's width  |
| `WorkspaceEvent`                 | What the person asked for (`NewRun`, `Say`, `Cancel`, `Fork`, `Land`, …), for the host to do |
| `update::HostUpdate`             | What a host tells the interface; `Workspace::apply` takes each one                           |
| `view::RunView`                  | A run as the interface shows it, built from `RunEvent`s with no window                       |
| `catalog::Catalog`               | What the workspace shows beyond runs: plugins, repositories, the store                       |
| `plugins`                        | The plugins in order (`plugins::plugins`), UI halves only, and `install` for a host's        |
| `remote`                         | The phone's side: `remote::connect` pairs with a computer and applies what it sends          |
| `attention`                      | Whether a chat needs the person: `Attention::of` over a run's `Facts`                        |
| `setup`, `pairing`, `phones`     | Onboarding, a phone's pairing, and the computer's list of phones                             |
| `pull_request`, `push`, `queue`  | Pull requests, pushing main, and the landing queue, as the host reports them                 |
| `models`, `picker`, `plan_usage` | Choosing models and effort, and what a ChatGPT plan error says                               |
| `route`, `search`, `slash`       | The screens, the search palette (Ctrl K), and slash commands in the composer                 |
| `titles`                         | A run's title, written by a model from its first prompt                                      |
| `ui`                             | The pieces screens are built from, and the screens themselves (`ui::screens`)                |
| `init`                           | Sets up fonts, the theme and the keys, once per app                                          |

It re-exports `tau-ui-kit`'s `assets`, `input`, `markdown` and `theme`.

## How it fits

It builds on `tau-agent` (with `serde`), `tau-ai`, `tau-store`,
`tau-remote`, `tau-terminal`, `tau-ui-kit`, `tau-ui-plugin`, and the UI
half of every plugin. It depends on no plugin's host half: those live in
`tau-*-host` crates, or behind the `host` feature for tau-ask, tau-direnv
and tau-skills, which it turns off. So a phone builds it without the tools,
Luau, jj-lib or an MCP client.

`tau-ui` drives it on the computer, `tau-phone` on Android, and the `tau`
app installs its plugins with their host halves.

The wiring goes two ways:

- In: every `HostUpdate` a host has goes to `Workspace::apply`: run events,
  the catalog, what plugins answered.
- Out: the workspace emits a `WorkspaceEvent` when the person starts,
  steers, cancels or forks a run. The host subscribes and acts on it.

## Usage

A window with a workspace, wired to a host:

```rust
use gpui::{App, AppContext as _, WindowOptions};
use tau_ui_remote::{Workspace, WorkspaceEvent, catalog::Catalog};

fn open(cx: &mut App) {
    tau_ui_remote::init(cx);
    let window = cx.open_window(WindowOptions::default(), |window, cx| {
        cx.new(|cx| {
            Workspace::new("tau", Vec::new(), Catalog::default(), window, cx)
        })
    });
    let Ok(workspace) = window.and_then(|window| window.entity(cx)) else {
        return;
    };
    cx.subscribe(&workspace, |_, event: &WorkspaceEvent, _| match event {
        WorkspaceEvent::NewRun { prompt, repo, .. } => {
            // Start a run on `prompt` in `repo`, then hand each event
            // it streams to `workspace.apply(HostUpdate::Event(e), cx)`.
        }
        WorkspaceEvent::Cancel { run } => { /* cancel it */ }
        _ => {}
    })
    .detach();
}
```

The app must serve the icons: build it with
`.with_assets(tau_ui_remote::assets::Assets)`. `tau_ui::host` is the full
wiring for a real coding agent, and `tau-phone`'s `android_main` the one for
a phone.

## Testing

```sh
cargo nextest run --release -p tau-ui-remote
```

None of them opens a window. `tests/design.rs` keeps raw design values out
of every file but `ui/components.rs`. `tests/attention.rs` checks
`Attention::of` against a small reference with Hegel, and
`tests/markdown.rs` reads replies back as blocks. `tests/sidebar.rs`,
`tests/terminal.rs`, `tests/table_layout.rs` and `tests/own_repo.rs` drive
the workspace in GPUI's test app: the sidebar, `bash` cards, tables and
tau's own repositories. Unit tests sit in `src/`.

## Further reading

- [docs/reference/attention.md](../../docs/reference/attention.md)
- [docs/reference/plugins.md](../../docs/reference/plugins.md), "A plugin's
  UI"
- [docs/decisions/0007-gpui-interface.md](../../docs/decisions/0007-gpui-interface.md)
- [docs/decisions/0013-phones-connect-to-a-running-tau.md](../../docs/decisions/0013-phones-connect-to-a-running-tau.md)
- [docs/decisions/0017-plugins-bring-their-ui.md](../../docs/decisions/0017-plugins-bring-their-ui.md)
- [docs/decisions/0030-host-halves-are-crates.md](../../docs/decisions/0030-host-halves-are-crates.md)
