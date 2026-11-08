# 0051: tau's Luau plugins are packages, not crates

- Status: accepted.
- Date: 2026-11-08

## Context

ADR 0048 and ADR 0050 made each built-in Luau plugin a "carrier": a Rust
crate whose `src/lib.rs` holds `NAME`, a `FILES` list of `include_str!`s,
sometimes `DEMO_*` fixtures, and whose `tests/` load the Luau. Eleven
exist now: tau-ask, tau-reasoning, tau-goal, tau-titles,
tau-compaction-chain, tau-fast-compaction, tau-todo, tau-constitution,
tau-compaction, tau-tree-compaction and tau-watcher. Three more
(tau-schedule, tau-canvas, tau-github) are on the way.

The crate exists only to get Luau into the binary, and Cargo rebuilds
everything downstream of it. Measured on 2026-11-08: touching one
`.luau` today rebuilds plugin crate → tau-luau-plugins-host → tau-ui →
tau because of `include_str!` in `FILES`: app 20 s wall / 238 s CPU;
`--workspace --all-targets` 104 s wall / 2,408 s CPU, 20 crates
recompiled.

The manifests give the edges behind that:

- tau-luau-plugins-host depends on all eleven, and tau on it.
- tau-demo, a normal dependency of tau, depends on eight of them for
  `NAME` and fixtures.
- tau-ui has tau-ask, tau-goal and tau-reasoning as normal dependencies
  that its `src/` never names. The other seven are dev-dependencies, as
  tau-luau-plugins-host is: "tau-ui does not link Luau" (ADR 0050).
- tau-agents, an interface half, depends on tau-ask for one string
  (`without: &[tau_ask::NAME]`).
- tau-ui-remote's dev-dependencies and three evaluations name them too.

A clean build should gain only modestly: the carriers hold 20 of about
293 test files. The gain is in the edit.

A person's plugin is a different shape again: `plugin.luau`, `lib/`,
`tests/` and a README, in the plugins repository (ADR 0027), and an agent
another: `agent.luau`, `lib/` and `tests/`, in a repository of its own
(ADR 0037). Lux's
packages show a plainer form, a manifest and a `src/`. It inspires this
layout. It is not a dependency, and nothing here reads Lux's format.

## Decision

Built-in Luau plugins stop being Rust crates and become plain packages
under `plugins/` at the repository's root. The Nix package installs them
to `share/tau/plugins` beside the binary, and tau reads them at startup.
tau is no longer one self-contained file. A person's plugin and an agent
have the same layout, and the old ones are gone: no users, no migrations.

This lands before tau-schedule, tau-canvas and tau-github are ported, so
they start in the new layout.

### A package

```
plugins/tau-goal/
  tau.toml             -- what the package is
  README.md            -- for the person, if it has one
  src/plugin.luau      -- the entry: returns tau.plugin { ... }
  src/goal.luau        -- a module: require("goal")
  tests/command.luau   -- tests, run against fake runs
  fixtures/met.json    -- data only Rust tests and the demo read
```

`plugins/built-in.toml` lists the built-ins, in order (see Decisions).
`crates/plugins/` keeps the Rust plugins. `plugins/` has no `Cargo.toml`,
is not a workspace member (`members` globs `crates/...`), and Cargo does
not look at it.

An agent is the same, its repository being the package: `tau.toml`,
`src/agent.luau`, `src/<module>.luau`, `tests/`.

### `tau.toml`

```toml
name = "tau-compaction-chain"
version = "0.1.0"
description = "Compacts with fast pruning, then the tree, then the summary, each declining when it would save too little"
entry = "src/plugin.luau"
```

Four fields. `entry` may be left out, for `src/plugin.luau` in a plugin and
`src/agent.luau` in an agent; the other three are required. Any other field
is an error, so a misspelling never passes as absent. The host half reads it with the `toml` crate
already in the workspace. Each check is a load error, which for a
repository's plugin is a Broken row saying why:

| Field         | Rule                                                                                                           |
| ------------- | -------------------------------------------------------------------------------------------------------------- |
| `name`        | Equal to the folder's name, as `load` requires of a declaration's name now; a folder's rule is `plugin_test`'s |
| `version`     | `MAJOR.MINOR.PATCH`, digits only, read by hand                                                                 |
| `description` | Not empty, one line                                                                                            |
| `entry`       | `src/<stem>.luau`, which exists; no `..`, no further folder; `<stem>` is not `tau`                             |

`name` and `description` live only here. `tau.plugin {}` and `tau.agent {}`
take neither, passing either is an error, and the host fills the
declaration from the manifest. The loader tells an agent from a plugin as
it did: by the `Root` its caller reads the folder with, and so which module
the entry is required as (`plugin` or `agent`).

`version` is for the person: `Entry` carries it and the Plugins screen
shows it. Nothing activates on it. The digest still names a version
(`Loaded::digest`, `Allowed::digest`), so a change that leaves `version`
alone is still a new version. Lux's dependencies and build fields have
no use here: a plugin's `uses`, hooks and tools stay in its Luau.

### `require`, `tests/` and `fixtures/`

- **`require` is by name, flat, within `src/`.** `require("goal")` is
  `src/goal.luau`, and `require("tau")` is the host's module. `Files`
  keeps its shape: the entry is `source`, every other `src/*.luau` is
  in `libs`, and `Files::modules` and its cycle check stay as they are.
  The entry is always the module `plugin` (`agent` in an agent), whatever
  its file is called, so the scripts that load it and `tau.test._plugin`
  do not change; a module beside it may not take that name or `tau`
  ("is taken", as now).
- **A `.luau` file in a folder under `src/` or `tests/` is an error.**
  `from_paths` and `read` skip them silently today, which reads as code
  that never runs. No carrier nests.
- **`tests/*.luau`** are what `plugin_test`, trunk's reload and
  `--plugin-test` run. Nothing changes in how, or in `TEST_LIMIT` and
  `PROPERTY_LIMIT`.
- **`fixtures/`** is data. `load` does not read it and a Luau test cannot
  (a test's VM has no files). The demo and Rust tests read it; a person's
  plugin may keep it or not. A non-UTF-8 file there does not fail a load,
  as `from_paths` decodes only the paths it recognizes.

### What a version's digest covers

What tau reads and nothing else, as now: the manifest's fields (so a
comment in `tau.toml` is no new version), the entry, each `src/` module, each test and `README.md`, each under its path in the
folder. `fixtures/` stays out, so a blessed fixture is not a new version
to load and test, and `in_workspace` and `ready` do not retry for one.

Every digest changes once. An `allowed.json` that no longer matches
falls to `grown_from`, which compares declarations, so a plugin reaching
what the person allowed stays allowed.

### The repository's plugins

The rest of ADR 0027 and 0034 stands. Three places read the layout:

- **`reload` (trunk).** Each top-level folder with files is still a
  plugin, through `Files::from_paths`. One with no `tau.toml` is a Broken
  row: "the folder has no tau.toml". So a repository still in the old
  layout shows every plugin Broken until it is rewritten, and there is
  no reader for the old layout.
- **`plugin_folders` (a run's workspace, `in_workspace`, `ready`).** It
  looks for `tau.toml` where it looked for `PLUGIN_FILE`, and reads with
  `Files::read`.
- **`testing::test` (`plugin_test`).** It reads through `Files::read`,
  and so follows.

`first_files`' README and AGENTS text, `SKILL.md`, the six plugins under
`skill/examples/`, `tests/skill.rs`, the module docs in
`tau-luau-plugins` and `docs/reference/plugins.md` all move to the new
layout in the same commit, since the skill teaches what the host reads.

An agent's repository reads the same way: tau-agents-host's `load` takes
`Files::from_paths(paths, AGENT)`, a commit without `tau.toml` is a version
that does not load (the version before stays live, and the agent hears
why), and its `first_files`, skill, five examples, tests and docs move
with it. There is no reader for an old agent's layout either.

### How tau finds the folder

`tau_luau_plugins::packages::find`, in the interface half because the
demo needs it and does not link the host half. It takes its inputs as
arguments, so a test can draw them. In order:

1. **`TAU_PLUGINS_DIR`, when set and not empty.** It is the folder. If it
   is not a directory, that is the error: it never falls through, since
   someone who set it wants that one.
2. **Installed: `<exe's dir>/../share/tau/plugins`**, from
   `std::env::current_exe`. In the Nix package `bin/tau` is a
   `wrapProgram` script that runs `bin/.tau-wrapped` by its store path,
   so the executable is in `$out/bin` whether tau was started through a
   profile's symlink or not.
3. **The source: `concat!(env!("CARGO_MANIFEST_DIR"), "/../../../plugins")`,**
   compiled into `tau-luau-plugins`. It serves `cargo run`, a binary run
   directly from `target/` (`scripts/readme-screenshots.sh`), every test
   binary, and the evaluations. `target/release/../share` does not exist,
   so step 2 passes these by.

Three other answers do not work here. Walking up from the executable
finds the wrong checkout when workspaces share a `CARGO_TARGET_DIR`,
which they do. Reading `CARGO_MANIFEST_DIR` at run time fails for a
binary started directly. And the wrapper does not set `TAU_PLUGINS_DIR`,
because it would pass to everything tau runs: tests a chat runs in tau's
own repository would read the installed plugins, not the ones they test.

A Nix build's own tests need nothing more: its check phase runs in
`/build/source`, where step 3 finds `plugins/` as long as the source
fileset includes it.

**Phones and the remote read nothing.** tau-phone and tau-ui-remote link
no plugin crate (`core_names_no_plugin.rs`). A phone gets trees and
catalogs from the computer (ADR 0046). The APK's source fileset needs no
change.

**A run outside Nix** has `cargo run`, or an install of its own with the
same `bin/` and `share/tau/plugins` shape, or `TAU_PLUGINS_DIR`. A binary
copied to another machine alone does not start agents, and says so.

**Sandboxes do not come into it.** The host process reads the folder,
outside `bwrap`: only an agent's commands run in one (ADR 0037). A
plugin's VM gets its modules as strings and has no files.

### What happens when it is missing or broken

A built-in provides core behaviour, so **no agent runs without all of
them**. Today a failure is close to silent: `load_built_in` prints one
line and goes on (its doc says the plugin is "listed as broken", and it
is not), and even an error from `PluginHost::new` only reaches
`host_all`, which prints "is off" and leaves tau-luau-plugins out. A
person who started tau from the menu would lose compaction and see
nothing.

The failure takes the path main already has for a host that cannot
start:

```rust
let host = (!args.demo).then(|| {
    tau::load_built_in_plugins()?;   // new: find, read, check the list, load
    Host::new(config)
});
```

`Some(Err(error))` in `main` prints to stderr and opens the window with
the alert "tau cannot start agents" and the error, so it shows from a
menu too. The error names the paths tried, or the plugin and its load
error.

`load_built_in_plugins` loads each once and keeps the result for the
process: `Loaded` is plain data, so `Registry` takes the same values when
it starts, with no second load. `Registry::new` returns an error if a
load failed, where `load_built_in` swallowed it. A test that builds its
own registry with `with_built_in` goes on as it does.

**A plugin that is partly written stops `cargo run`.** That is the
loudness asked for, seen from the other side: `--plugin-test` says what
is wrong without starting anything.

The demo starts no host. It reads its fixtures when a screen asks, and a
missing one panics, naming its path.

### Binary and folder from different builds

**No version check, and nothing catches it.** `plugins/built-in.toml`
travels with the folder, so it says what that folder holds, not what this
binary expects. If the two come from different builds, the file and the
folder still agree, and tau starts.

In the Nix package they are one store path and cannot differ.
`TAU_PLUGINS_DIR` is someone asking for it. From a checkout they do differ,
as after a `jj edit`, and the checkout's Luau is what a developer wants to
run then. A stamp would need a `build.rs` watching `plugins/`, which is the
rebuild this ADR removes.

So a mismatch shows only as what it breaks: a plugin that calls a `tau.*`
this binary lacks fails at load or in a hook, with its error, as any
plugin's does. A plugin that relies on a change in what a hook is given
may misbehave with no error. The loading checks are about a broken folder
(a listed plugin missing, a package not listed), not an old one.

### What replaces each const

| Now                                                                       | After                                                                                  |
| ------------------------------------------------------------------------- | -------------------------------------------------------------------------------------- |
| `tau_luau_plugins_host::BUILT_IN`, `&'static [BuiltIn]`, in rewrite order | `plugins/built-in.toml`, the order and what must be there                              |
| `BuiltIn { name, files: &'static [...] }`                                 | `BuiltIn { name: String, files: Files }`, read at startup                              |
| `Registry::with_built_in(&'static [BuiltIn])`                             | `with_built_in(Vec<Loaded>)`; `tests/registry.rs`'s const becomes a function           |
| `Shared::built_in: &'static [BuiltIn]`                                    | `Vec<BuiltIn>`, and `reload`'s check of a taken name reads it                          |
| `<plugin>::NAME`                                                          | `tau_luau_plugins::names::{ASK, GOAL, ...}`, plain strings; not the list               |
| `<plugin>::FILES`                                                         | `Files::read(find()?.join(name), PLUGIN)`                                              |
| `Active::built_in(name)`, from `BUILT_IN`                                 | The same call, read from the listed folder; still `Result`                             |
| `tau_ask::DEMO_ASK`, `DEMO_NOTE`, `DEMO_REVIEW`, `DEMO_SWEEP`             | `fixtures/ask.json`, `note.json`, `review.json`, `sweep.json`                          |
| `tau_reasoning::DEMO_CHOSE`, `DEMO_KEPT`                                  | `fixtures/chose.json`, `kept.json`                                                     |
| `tau_goal::DEMO_WORKING`, `MET`, `STOPPED`, `TRIAGE`, `LANES`             | `fixtures/working.json`, `met.json`, `stopped.json`, `triage.json`, `lanes.json`       |
| `tau_goal::DEMO_GOAL`, text in Rust                                       | A literal in `tau_demo::GOAL`, and in the fixture's test, which checks it either way   |
| `tau_constitution::DEMO_RUN`, `DEMO_PAGES`                                | `fixtures/run.json`, `pages.json`                                                      |
| `tau_fast_compaction::DEMO_RUN`                                           | `fixtures/run.json`                                                                    |
| `tau_watcher::DEMO_NEW`, `DEMO_LEARNED`, `DEMO_KNEW`                      | `fixtures/new.json`, `learned.json`, `knew.json`                                       |
| `DEMO_DECLARATION` (seven plugins)                                        | `fixtures/declaration.json`                                                            |
| `tau_fast_compaction::HEADER`, a copy of a line the Luau writes           | `tau_luau_plugins::names::PRUNED_HEADER`; the new test crate checks the Luau writes it |
| `runtime::PLUGIN` (`file: "plugin.luau"`), `PLUGIN_FILE`                  | `PLUGIN` is a module and a default entry; the entry's path comes from `tau.toml`       |

The demo "cannot run Luau", and that stays true: it gets its fixtures
through `tau_luau_plugins::packages::fixture(plugin, name) ->
Result<String, String>`, so it has no Rust dependency on a plugin.
`tau_demo::luau(plugin, fixture)` and `luau_step` take the fixture's name
and read it. `screens.rs`'s `match`es on a screen pick a name in place of
a const. Everything else about the fixtures is as it was: `{ state, view }`,
checked by Rust against what the plugin draws, `BLESS_FIXTURES=1` to write
them. Bless writes through `find`, so it writes where tests read.

What was `&'static str` becomes `String` at these edges and nowhere
else, and the `load` messages that said `PLUGIN_FILE` say the entry's path.

### Dependencies that go

| Crate                     | Loses                                                                                                                     |
| ------------------------- | ------------------------------------------------------------------------------------------------------------------------- |
| `tau-luau-plugins-host`   | all eleven                                                                                                                |
| `tau-demo`                | tau-ask, tau-compaction, tau-tree-compaction, tau-constitution, tau-fast-compaction, tau-goal, tau-reasoning, tau-watcher |
| `tau-ui`, normal          | tau-ask, tau-goal, tau-reasoning, unused by `src/`                                                                        |
| `tau-ui`, dev             | tau-watcher, tau-titles, tau-constitution, tau-compaction, tau-tree-compaction, tau-fast-compaction, tau-todo             |
| `tau-ui-remote`, dev      | tau-compaction, tau-fast-compaction, tau-reasoning, tau-ask, tau-goal                                                     |
| `tau-agents`              | tau-ask, for `names::ASK`                                                                                                 |
| `tau-memory-e2e`          | tau-compaction                                                                                                            |
| `tau-rlm-eval`            | tau-compaction                                                                                                            |
| `tau-output-pruning-eval` | tau-fast-compaction                                                                                                       |
| The workspace             | the eleven `[workspace.dependencies]` lines, and `crates/plugins/<each>`                                                  |

`tau-memory-e2e` and `tau-rlm-eval` gain tau-luau-plugins for the name;
they link its host half already.

### Where the Rust tests go

They collapse into one crate, `crates/tau-built-in-tests`, with **one
test binary** (`tests/main.rs`, a module per row, `autotests = false`),
so 20 links become one. Its dev-dependencies are the union of the
carriers': tau-agent, tau-ai, tau-codemode-host, tau-jev,
tau-luau-plugins, tau-luau-plugins-host, tau-store, tau-store-sqlite,
tau-testing, async-trait, futures-util, hegeltest, jsonschema, schemars,
serde, serde_json, tempfile, tokio and tokio-util.

| From                 | Files (binaries)                                                                 |
| -------------------- | -------------------------------------------------------------------------------- |
| tau-ask              | `luau`                                                                           |
| tau-reasoning        | `luau`, `replay`; `examples/replay.rs` stays an example here                     |
| tau-goal             | `luau`                                                                           |
| tau-titles           | `luau`                                                                           |
| tau-compaction-chain | `luau`                                                                           |
| tau-fast-compaction  | `luau`, `in_the_loop`, `output_in_the_loop`, with `common/`                      |
| tau-todo             | `luau`, `store_model`, `crash_model`, `beads_oracle`, `schemas`, with `support/` |
| tau-constitution     | `luau`                                                                           |
| tau-compaction       | `luau`, `in_the_loop`                                                            |
| tau-tree-compaction  | `luau`, `in_the_loop`                                                            |
| tau-watcher          | `luau`                                                                           |

Each `luau.rs` keeps its name and its checks. The loading and
bless-or-check code each repeats becomes one helper. Whatever a test
reads comes through `find`, never `include_str!`.

`tau-ui/tests/ask.rs` and `watcher_ui.rs`, which read `FILES` to load a
plugin, read it through `Files::read`; they keep tau-luau-plugins-host,
which tau-ui has as a dev-dependency already. The other tau-ui and
tau-ui-remote tests only need the names.

The two checks that the kernel names no plugin
(`tau-ui-remote/tests/core_names_no_plugin.rs`, `tau-ui/tests/kernel.rs`)
list plugins by reading `crates/plugins`. Left as they are, they would
stop seeing the eleven, and the kernel could name `"tau-goal"` unnoticed.
Both read `plugins/` as well.

### After this, a Luau edit rebuilds nothing

Nothing Cargo tracks points at `plugins/`: no crate has it in its package,
no `include_str!` reaches it, and neither `build.rs` looks there
(`tau-skills` watches its own `skills/`, `tau-ui-kit` its own `assets/`).
A new test in the new crate fails if any Rust source under `crates/` has
`include_str!` or `include_bytes!` of a path into `plugins/`, or if a
`build.rs` names it.

Editing these still rebuilds, and should, as they are not plugin code:

- `tau-luau-plugins/src/tau.luau` (`TAU_MODULE`): the interface itself,
  embedded, and part of the binary it must match.
- `tau-luau-plugins-host/skill/SKILL.md` and its `examples/`, and
  `tau-agents-host/skill/`, through `include_str!`.
- `tau_luau_plugins::names`, if a change makes Rust name a new plugin. A
  new built-in on its own needs no line of Rust: its folder and a line in
  `plugins/built-in.toml`.

### Running a package's tests without compiling

`tau`'s command line is flags read in `main`; it has no subcommands, and
the only way to run a plugin's tests now is `plugin_test`, a tool in a
run. Two flags join them, both handled before a window or a host exists:

- **`tau --plugin-test <folder>`** reads the folder as `plugin_test`
  does (`testing::test`, made public, on `Files::read`), prints what the
  tool prints, and exits 0 only if it loads and every test passes. Tests
  run on fake runs, and hegel's engine is linked in, so it needs no
  model, network or display. After one `cargo build --release -p tau`,
  `target/release/tau --plugin-test plugins/tau-goal` runs any later edit
  of that Luau with no compiling.
- **`tau --check-plugins`** does what startup does
  (`load_built_in_plugins`), prints the folder and the plugins it loaded,
  and exits 1 with the same error otherwise.

`--plugin-test` does not check fixtures, which needs Rust to drive hooks.
`BLESS_FIXTURES=1 cargo nextest run --release -p tau-built-in-tests` does
that, and compiles nothing after a Luau edit either.

### Nix

The `tau` package in `flake.nix` needs three things:

```nix
src = pkgs.lib.fileset.toSource {
  root = ./.;
  fileset = pkgs.lib.fileset.unions [
    ./Cargo.toml
    ./Cargo.lock
    ./crates
    ./plugins
  ];
};

postInstall = ''
  mkdir -p $out/share/tau
  cp -r plugins $out/share/tau/plugins
  # ...the icons, as now
'';

doInstallCheck = true;
installCheckPhase = ''
  runHook preInstallCheck
  $out/bin/tau --check-plugins
  $out/bin/tau --plugin-test $out/share/tau/plugins/tau-titles
  test -f $out/share/tau/plugins/tau-goal/fixtures/working.json
  runHook postInstallCheck
'';
```

- **`./plugins` stays a path in the fileset, and `cp` reads the unpacked
  source.** No `toString ./plugins` and no `"${./plugins}"`: under lazy
  trees `toString` copies the whole tree, `target/` included.
- **The whole folder is installed**, `fixtures/` because `tau --demo`
  reads them and `tests/` so `--plugin-test` works on what is installed.
- **The install check is what proves the package has the plugins.** It
  runs after `postFixup`, on the wrapper, so it exercises step 2 of how
  tau finds the folder. `--check-plugins` opens no window, so it needs no
  display in the build's sandbox.
- `nix/tau-phone.nix` does not change.

### Order

Each is one commit, landable on its own: `nix fmt`, workspace clippy with
`--all-targets`, `cargo nextest run --release --workspace`.

1. **tau-ui's unused normal dependencies** on tau-ask, tau-goal and
   tau-reasoning become dev-dependencies. Nothing else.
2. **The package layout, still embedded.** `tau.toml` and its checks,
   `Files`' new layout and digest; the eleven move to `plugins/<name>/`
   (`luau/plugin.luau` to `src/plugin.luau`, `luau/lib/` into `src/`,
   `luau/tests/` to `tests/`, `fixtures/`, `README.md`, and tau-fast-compaction's
   `THIRD_PARTY_NOTICES.md`), each given a `tau.toml`. The carriers'
   `FILES` and `DEMO_*` `include_str!` the new paths, so `flake.nix`'s
   fileset gains `./plugins` here. The repository's plugins, the skill,
   its examples, `first_files` and the docs follow. So do agents:
   tau-agents-host, its skill, examples, `first_files` and tests, as `Files`
   has one layout. `name` and `description` leave `tau.plugin {}` and
   `tau.agent {}`.
3. **Read at startup.** `packages::find`, `plugins/built-in.toml`, `BuiltIn`
   owned, `Active::built_in`, `load_built_in_plugins` and the alert path,
   `--plugin-test`, `--check-plugins`. tau-luau-plugins-host drops its
   eleven dependencies. **`flake.nix`'s `postInstall` and install check
   come in this commit**, or the package breaks.
4. **`crates/tau-built-in-tests`.** The 20 binaries move, read through
   `find`, and the carriers lose their tests.
5. **The dependents.** tau-demo reads fixtures; tau-ui, tau-ui-remote,
   tau-agents and the evaluations use `names`.
6. **The crates go.** The eleven and their workspace lines; both kernel
   checks read `plugins/`; the guard against `include_str!` joins; the
   reference docs and the root `THIRD_PARTY_NOTICES.md` point at the new
   paths. The numbers below go in this ADR's "As built".
7. **tau-schedule, tau-canvas and tau-github** port as ADR 0050 lays out,
   into `plugins/`.

## Testing

- **`find`**: on drawn combinations of override, executable and source
  folders that exist or not, the answer follows the order, an override
  that is no directory is an error whatever else exists, and the error
  names every path tried.
- **`tau.toml`**: a table of bad manifests, each with its message:
  missing field, unknown field, a `name` not the folder's, `version` not
  three numbers, an `entry` outside `src/`, with `..`, or missing. A
  property: `read` and `from_paths` give the same `Files` and digest for
  any drawn package, and the digest changes with any file but those in
  `fixtures/`.
- **The built-ins**: `plugins/built-in.toml` and `plugins/` hold the same
  names; each `tau.toml`'s `name` is its folder's; the file keeps fast
  pruning, the tree and the summary in that order; each `luau.rs` passes.
- **Failing loudly**: a folder missing a listed plugin, one holding a
  package the file does not list, and one with a broken plugin, make `load_built_in_plugins` an error; `tau --check-plugins`
  against `TAU_PLUGINS_DIR` pointing at each exits 1 and names the
  problem.
- **The repository**: `tests/registry.rs`, on the new layout: a folder
  without `tau.toml` is Broken, a workspace's version activates as before,
  a built-in's name stays taken.
- **The package**, cheaply and on every run: a text check of `flake.nix`
  that its fileset names `./plugins`, its `postInstall` the install path,
  and its install check `--check-plugins`, and that it has no `toString`
  of a path. The real check is the install check itself.

### What to measure after

No numbers are claimed for the new layout, as none exist. In "As built",
against the figures in the Context, on the same machine:

- One `.luau` touched: `cargo build --release -p tau`, and
  `--workspace --all-targets`, wall and CPU, and crates recompiled. Zero
  crates is the claim to test.
- A clean build, wall and CPU. Expect modest.
- The test binaries' link time, and `tau-built-in-tests` against the 20.
- Startup: from exec to `Host::new` returning, cold and warm cache, with
  the read and the load apart. The eleven's Luau and fixtures are 85
  files and 919 KB today, 397 KB of it entries and modules.
- `tau --plugin-test` on the largest plugin, and the full
  `tau-built-in-tests` run.
- The package's closure, and the binary's size.

## Alternatives considered

- **Embed them with a generated list.** A `build.rs` walking `plugins/`,
  as tau-skills does, keeps one file, but needs `rerun-if-changed`, and an
  edit rebuilds the same chain. Embedding only in release leaves two ways
  to run, and "the tests are in release" here. The owner chose not to
  embed, knowingly.
- **Keep the crates and split out only the fixtures.** It fixes tau-demo's
  edge and leaves tau-luau-plugins-host's, and that is the costly one.
- **Lux itself, with its manifest.** Its dependencies, builds and rockspecs
  answer questions these packages do not have, and tau would owe its
  format a compatibility it does not need.
- **Compile a Nix store path into the binary.** It needs the binary to
  refer to its own output or a second derivation, for what
  exe-relative gets without either.
- **Keep starting with what loaded, and show a Broken row.** A person's
  plugin may be Broken, because the version before stays active and
  nothing depends on it. A broken compaction is an agent that overflows
  later and says why then.

## Consequences

Gains:

- A Luau edit should compile nothing, which is the point, and a port is
  Luau, a folder and tests, with no crate to add.
- Tests stop linking 20 binaries for these plugins.
- A person's plugin and tau's own have one layout, so reading one teaches
  the other.
- The binary drops the Luau and fixtures it carried.

Costs:

- **tau is no longer one file.** `target/release/tau` copied elsewhere,
  or a `nix bundle`, starts without agents unless the folder comes along,
  and the alert says so. This is the choice the owner made.
- **A source path is compiled into every binary.** In a Nix build it is
  `/build/source/...`, which does not exist on a normal machine. It is
  looked at only if step 2 failed.
- **What can write the folder gets a built-in's reach.** Built-ins are
  active with no Allow. In the store, nothing can write it. From a
  checkout, a run that edits `plugins/` in tau's own repository takes
  effect on the next `cargo run`, as editing Rust does. `TAU_PLUGINS_DIR`
  needs someone who can already run code as that person. No Allow stands
  in between in either case.
- **Startup does I/O** it did not, and reads every folder before the
  window. The bytes are small, on a path that already does a VM's work per
  plugin; the measurement above says whether it matters on a cold cache.
- **The running tau and the disk can differ.** Built-ins are read once at
  start, so a garbage collection or an upgrade does not reach a running
  tau, but the demo reads fixtures when a screen opens, and an old
  `tau --demo` whose store path was collected panics. Editing `plugins/`
  needs a restart to take effect, unlike a repository plugin.
- **A partial edit stops `cargo run`**, as above.
- **A person's existing plugins go Broken** until rewritten, with no
  reader for the old layout. No users, no migrations.
- **Packaging breaks can land unseen.** CI does not build the package, by
  decision, and the install check runs only on a hand-run `nix build`. A
  package without its plugins, or one that fails to build, can reach `main`
  and stay there until someone builds it. The text check of `flake.nix` in
  the test suite catches only the mistakes it spells out.
- **`--plugin-test` needs a built `tau`**: the first run compiles it, the
  rest do not.

## Decisions

The owner settled what the code did not, on review.

1. **The list and order live in `plugins/built-in.toml`.** `BUILT_IN` is an
   order as well as a list: "the first rewrite winning: fast pruning, then
   the tree, then the summary". A folder reads alphabetically, which would
   put the summary first and change what compaction does. Adding a built-in
   compiles nothing. Loading fails if a listed plugin is missing or the
   folder has a package the file does not list. Where Rust must name one
   plugin (tau-agents' `without`, the demo, tests), it uses a plain string
   from `tau_luau_plugins::names`, which is not the list. A test keeps fast
   pruning, the tree and the summary in that order.

2. **`name` and `description` live only in `tau.toml`.** `tau.plugin {}`
   takes neither, passing either is an error, and the host fills the
   declaration from the manifest.

3. **Agents convert too.** An agent's folder takes the same layout, so there
   is one layout, and no switch in `Root`. Decision 2 holds for `tau.agent
{}`. `entry` defaults to `src/agent.luau` there. No reader for the old
   layout, no migrations.

4. **CI does not build the package.** No CI step. The text check of
   `flake.nix` stays in the test suite, and the install check runs only on a
   hand-run `nix build`. Costs says what that risks.
