# 0007: An optional GPUI interface, outside the library

- Status: proposed. Amended by
  [0017](0017-plugins-bring-their-ui.md): plugins draw their own part of
  the interface, through `tau-ui-plugin` and `tau-ui-kit`, not `tau-ui`.
- Date: 2026-09-28

## Context

[0001](0001-library-not-product.md) keeps tau-agent a library, with no
CLI, TUI or server. Watching runs from code alone is hard, though:
streams of events, forks, sub-agents, and plugins that change a run
without saying so in the transcript. We want an interface for people
to start, steer and read runs, including what each plugin decided.

The interface must not turn the library into a product. The library's
users should not pay for a UI they do not use, and the UI must not
reach into the loop.

## Decision

- **`tau-ui` is its own crate** in `crates/tau-ui`. No core crate or
  plugin depends on it. It depends on `tau-agent` and `tau-ai` and uses
  only their public API, as any host program would.
- **GPUI draws it.** It is Zed's framework: GPU-rendered (Metal,
  Vulkan, DirectX), keyboard-first, and native on macOS, Linux and
  Windows.
- **The crate draws runs; it does not start them.** A host program owns
  the agents and wires the workspace in two directions:
  - in: it forwards each `RunEvent` to `Workspace::apply_event`;
  - out: it acts on each `WorkspaceEvent` (new run, steer, cancel,
    fork, keep a note, keep a branch, run a query, and the onboarding
    and pull request requests below).
- **The view model is separate from GPUI.** `RunView` folds run events
  and can be tested without a window. What is not per-run (plugins and
  their seams, memory notes, constitution rules, the store) comes in
  as a `Catalog` from the host.
- **Plugin decisions no event carries yet come in as `RunUpdate`s**: the
  effort `tau-reasoning` chose, which rule blocked a call, the notes
  `tau-memory` suggests, the pruning ledger. A blocked call reaches the
  event stream as plain text today, with no sign of the plugin or the
  rule.
- **One design language, in two files.** `src/theme.rs` holds every
  token: colors, the type scale (`Type`), font weights, corner radii,
  icon sizes, control heights, and spacing in steps of one unit (`sp`).
  `src/ui/components.rs` holds the shared components (buttons by kind,
  badges, chips, tags, cards, panels, fields, notices). Screens compose
  them and never write their own sizes or colors; `tests/design.rs`
  fails the build if one does.
- **Models are chosen per message.** The composer's picker sets the
  model and reasoning effort of the next run, or of an open chat's next
  message, grouped by price, with what the sign-in cannot run shown
  locked. A run keeps its model while it works (the session sends only
  what is new); the next message may go to another model, which gets
  the whole conversation, reasoning included (checked on Codex across
  gpt-5.5, gpt-5.6 and gpt-6, 2026-09-28). A fork is for trying the
  task again, not for changing model. Defaults per agent, the models
  the picker shows, and a price to ask above are saved in
  `$XDG_CONFIG_HOME/tau/models.json`.
- **One layout per width, not per device.** Below 720 px the window
  gets a one-column phone layout with a tab bar and bottom sheets;
  below 1100 px the desktop layout drops the inspector.
- **Onboarding and pull requests are screens with seams too.** The
  setup flow (GitHub, a model, repositories, the first run) and the
  pull request screens hold a `Setup` and `PullRequest` view model.
  The workspace asks with `WorkspaceEvent`s (sign in, clone, prepare
  or create a pull request) and the host answers with
  `Workspace::update_setup`, `set_pull_request` and
  `set_pull_request_state`. The workspace moves to the next step when
  an answer completes one. These screens take the whole window,
  without the sidebar.
- **The bundled host does what the libraries can.** It signs in with
  ChatGPT in the browser, the only way to reach a model
  ([0012](0012-chatgpt-sign-in-only.md)), then starts agents. GitHub sign-in, clones and pull requests need a GitHub App
  that tau does not have yet. Until then onboarding starts at the
  model, and the pull request button only shows when the catalog says
  the host can open one.
- **A demo host ships with the crate.** `cargo run -p tau -- --demo`
  replays a scripted session through the same `RunUpdate` path a real
  agent uses, and answers onboarding and pull requests the way a host
  would. `--open <screen>` and `--phone` start on a screen or in a
  phone frame.

## Consequences

- 0001 still holds for the library: `tau-agent` has no UI, and a user
  who wants none depends on nothing new. The UI is a product-shaped
  crate beside the library, not inside it.
- Plugins should report their decisions as run events. Until
  `RunEvent` has a plugin event, hosts translate what they know into
  `RunUpdate`s, and a UI fed only run events shows less than the
  mockups do.
- GPUI brings a large dependency tree. `deny.toml` carries its license
  exceptions and unmaintained-crate advisories, each marked as coming
  from gpui.
- On Linux the app needs Vulkan, Wayland or X11, and xkbcommon at run
  time. The dev shell provides them; a packaged app must too.
- GPUI has no Android or iOS backend. The phone layout serves narrow
  windows until one exists (see "Phones" below).

## Phones

- **Agents do not run on the phone.** A phone app is a remote client:
  runs live on a desktop or a server, and the phone shows and steers
  them. That needs a protocol that carries `RunEvent`s and
  `RunUpdate`s to the phone and `WorkspaceEvent`s back. 0001 keeps an
  RPC server out of the library, so the protocol gets its own decision
  and its own crate.
- **No phone build for now.** The candidate for one is
  [gpui-mobile](https://github.com/itsbalamurali/gpui-mobile), which
  implements GPUI's `Platform` trait for iOS (Metal) and Android
  (Vulkan) through wgpu. Adopting it would mean:
  - its GPUI: longbridge's fork, `gpui-pre-mobile`, builds on
    `gpui-pre`, which tau-ui has used since 2026-09-30, pinned to the
    same exact version (see `docs/research/gpui-mobile.md`);
  - its license, a choice of GPL-3.0, AGPL-3.0 or Apache-2.0;
  - an Android build in the flake (SDK, NDK, and packaging an APK).
- The phone layout is already written against the same `Workspace`, so
  a mobile backend would ship the screens we have.
