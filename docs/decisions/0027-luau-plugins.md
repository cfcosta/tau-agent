# 0027: Luau plugins, written and rewritten through tau

- Status: proposed. Builds on [0005](0005-plugins.md),
  [0017](0017-plugins-bring-their-ui.md) and
  [0018](0018-codemode-and-mcp.md), unchanged for Rust plugins. Amends
  [0015](0015-a-main-chat-per-repository.md): tau keeps one local
  repository of its own, the plugins repository, beside those from
  GitHub.
- Date: 2026-10-05

## Context

Every plugin today is a Rust crate compiled into tau. A rule ("no
deploys on Friday"), a tool for one's stack, or a check before a run
stops needs a fork of tau, Rust and a rebuild.

The person should instead ask tau for a plugin, try it, and ask again
until it does what they want, as often as they like. So a plugin is
something an agent writes, tests and changes in a loop, and the
interface follows each change.

tau-codemode already runs Luau safely: a fresh VM per call, a memory
cap, a deadline and cancel interrupt, no files, processes or network,
and globals (`tools.*`, `jev.*`, `infer`, the store) that go back
through the run's loop, so rules and other plugins still see each
call. Its modules are content-addressed, tested with fake tool calls,
and promoted into a repository only with the person's approval.

Three things stand in the way:

- **The UI.** 0017 has every plugin bring its UI as arbitrary gpui.
  Luau cannot draw gpui, and the phone builds codemode without its
  host half, so it cannot run Luau at all.
- **Trust.** A plugin's code runs inside every later run. A run's
  model must not be able to change it behind the person's back: to
  turn off their rules, or read every tool result.
- **Iteration.** Approving every edit by hand would make rewriting a
  plugin ten times a chore; activating every edit silently would make
  the person's approval meaningless.

## Decision

### Plugins are files in tau's plugins repository

- tau keeps a local jj repository of its own, listed as `tau-plugins`,
  under `$XDG_DATA_HOME/tau/luau-plugins` (`plugins/` there holds Rust
  plugins' own data), in the sidebar like a repository from GitHub, with a main chat and chats under it. Nothing new is
  needed to edit it: runs there use the usual tools, commit, land and
  show their diffs.
- One plugin is one folder:

  ```text
  tau-plugins/
    no-friday-deploys/
      plugin.luau        -- the plugin: returns its table
      lib/*.luau         -- modules it requires, by name; each may
                         -- require only `tau`
      tests/*.luau       -- its tests
      README.md          -- what it is for, for the person and the model
  ```

- **A plugin is active at a commit of the repository's trunk.** Main's
  commits, and chats landing on main, are how a plugin changes, so
  every version is a reviewable change with a diff, and the operation
  log can undo it.

### The Luau interface

`plugin.luau` returns a table. The host loads it once per version in a
fresh VM to read what it declares; hooks then run in their own VMs.

```luau
local tau = require("tau")      -- the host's API, below
local ui = tau.ui               -- the view pieces

return tau.plugin {
  name = "no-friday-deploys",
  description = "Blocks deploy commands on Fridays.",

  -- What it may reach. The host refuses anything else, and a change
  -- here is what needs the person's click (see Activation).
  uses = { tools = { "bash" }, jev = false, infer = false },

  -- What the person can set; the Plugins screen draws a form from it.
  settings = {
    schema = { type = "object", properties = {
      days = { type = "array", items = { type = "string" } },
    }},
    default = { days = { "Friday" } },
  },

  -- Tools the model can call, declared on every run (0022).
  tools = {
    deploy_window = {
      description = "Whether now is a safe time to deploy.",
      parameters = { type = "object", properties = {} },
      call = function(args, ctx)
        return { ok = not tau.contains(ctx.settings.days, ctx.now.weekday) }
      end,
      -- Its card's body, from the call and its result.
      card = function(call, result, ctx)
        return ui.badge(result.ok and "safe" or "wait", result.ok and "good" or "warn")
      end,
    },
  },

  -- Before each tool call: allow (nil), block, or flag for review.
  before_tool = function(call, ctx)
    if call.name == "bash" and call.args.command:find("deploy")
       and tau.contains(ctx.settings.days, ctx.now.weekday) then
      return tau.block("No deploys on " .. ctx.now.weekday .. ".")
    end
  end,

  -- When the model would stop: stop (nil), or go on with a message.
  before_stop = function(stop, ctx)
    if not stop.text:find("tests") then
      return tau.continue("Say which tests you ran.")
    end
  end,

  -- Once a turn, after its tools: what happened, to count or record.
  turn_end = function(turn, ctx)
    ctx.state.turns = (ctx.state.turns or 0) + 1
  end,

  -- What the interface shows, from the run's state. Pure: no tools.
  view = function(state, ctx)
    return {
      status = ui.status("on", tostring(state.turns or 0) .. " turns"),
    }
  end,

  -- What a button in the view does.
  actions = {
    reset = function(args, ctx) ctx.state.turns = 0 end,
  },
}
```

**`ctx`**, given to every hook:

| Field          | What it is                                                                            |
| -------------- | ------------------------------------------------------------------------------------- |
| `ctx.run`      | `{ id, kind = "main" \| "chat" \| "sub_agent", repo, model, turn }`                   |
| `ctx.now`      | `{ unix, iso, weekday }`, the host's clock, so tests can fix it                       |
| `ctx.settings` | the person's settings, defaults filled in                                             |
| `ctx.state`    | the plugin's state for this run; what a hook changes is kept (below)                  |
| `ctx.tools`    | in a tool's `call`: the tools `uses` names, as codemode calls them, through the loop  |
| `ctx.jev`      | Jev, when `uses.jev` and the run has a key; charged to the run                        |
| `infer`        | codemode's global, when `uses.infer`; charged to the run                              |
| `ctx.log`      | `ctx.log(text)`: a line in the plugin's log on its page, never in the model's context |

**Hooks and what they return:**

| Hook          | Given                                  | Returns                                       | Time limit |
| ------------- | -------------------------------------- | --------------------------------------------- | ---------- |
| tool `call`   | the arguments                          | a JSON value, or `error(message)`             | 60 s       |
| tool `card`   | the call and its result                | a view tree                                   | 200 ms     |
| `before_tool` | `{ id, name, args }`                   | `nil`, `tau.block(why)`, `tau.flag(why)`      | 2 s        |
| `before_stop` | `{ text, turn, reason }`               | `nil`, `tau.continue(message)`                | 10 s       |
| `turn_end`    | `{ turn, text, calls = { name, ok } }` | nothing                                       | 2 s        |
| `run_end`     | `{ stop, turns, cost }`                | nothing                                       | 10 s       |
| `view`        | the state                              | `{ status?, note?, card?, page? }` view trees | 200 ms     |
| `actions[x]`  | the action's arguments                 | nothing                                       | 2 s        |

- **State.** `ctx.state` is a JSON table per run. After a hook, the host
  stores what changed as a plugin record (`PluginCtx::publish`), so
  live and stored runs agree and forks inherit it, as for Rust plugins.
- **Failures fail open, and say so.** A hook that errors or runs out of
  time counts as `nil` (allow, stop), and the plugin's status turns red
  with the error. Three failures in a run turn the plugin off for the
  rest of it. A plugin cannot hang or break a run.
- **`before_stop` is capped**: three continuations a run, as tau-goal's
  are, so two plugins cannot keep a run going forever.
- **Not in this interface yet:** changing a call's arguments, rewriting
  a result, rewriting the context, choosing the model or effort. They
  change what the model sees or break the prompt cache, and come later
  if at all.

### The view: a JSON tree, drawn by Rust

`tau.ui` (a field of `tau`: codemode's module names cannot hold a dot)
builds plain tables: `text`, `rich` (with `code` and
`**bold**`), `mono`, `badge(text, tone)`, `rows({ key = value })`,
`list`, `progress(share)`, `code(lang, text)`, `stack`/`row` to lay
them out, and `button(label, action, args)`. The host calls `view`
after each change of state, keeps the tree in the plugin's run state,
and one Rust renderer draws it on the computer and on the phone, which
never runs Luau. Trees are checked against the vocabulary; an unknown
piece draws as a red "unknown piece" box rather than failing.

Where trees go: `status` on the run's status line, `note` in the
transcript, `card` under the plugin's own tools' cards, and `page` on
the plugin's page.

### The interface follows each change

- **The registry watches trunk.** When the plugins repository's trunk
  moves, tau reloads the plugins that changed: it reads each one's
  declaration, runs its tests, and shows the outcome at once on the
  Plugins screen: active at which commit, tests passing or failing,
  waiting for a click, or broken (with the Luau error and line).
- **Views re-render live.** `view` is a pure function of state, so when
  a plugin's version changes, the host re-renders the trees of every
  run on screen with the new version, stored runs included. Status
  lines, notes, cards and pages change while the person watches.
- **Behaviour is pinned per run.** A run keeps the hooks and tools it
  started with: its tools are fixed for the run (0022), and a hook
  changing in the middle would make it inconsistent. The next message
  to a chat starts a run on the new version; a running chat says
  "plugin updated: applies from your next message".
- **Settings forms** come from `settings.schema` and follow it as it
  changes; values the new schema rejects are dropped, and the form says
  which.
- **Errors are where the plugin is**: on its page (the log, the failing
  test, the hook that failed and its error), and as a red status line in
  any run it failed in, linking to the page.

### Activation: one click for new reach, none for the rest

A version activates when trunk reaches it and its tests pass, unless it
needs the person:

- **Its `uses` grew** (a new tool, Jev, `infer`) or it gained a seam it
  did not have (`before_tool`, `before_stop`): the Plugins screen and
  the chat that changed it show "no-friday-deploys wants to use `bash`
  · Allow", with the diff of its declaration. Until then the previous
  version stays active.
- **Otherwise** it activates by itself. The person already chose the
  change by landing it or by talking to main, and it can reach nothing
  it could not before.
- **Tests failing**: it does not activate; the previous version stays,
  and the failure shows on its page and in the chat.

A version is named by its commit and the digest of its folder; the
person can pin a plugin to a version, or roll back to one, from its
page.

**Plugins from other repositories** (a team's `.tau/plugins` folder)
never activate by themselves: they arrive through codemode's
promotion, approved by digest, and are copied into the plugins
repository as a change of their own.

### Writing one: a skill tau ships, and tools to test

- **A built-in skill, `tau-plugins`**, is listed in every run: the
  interface above, the view pieces, three worked plugins (a rule, a
  tool with a card, a stop check), how to test, and how the person sees
  the result. A skill of the same name in `~/.agents/skills` replaces it.
  tau-skills gains built-in skills, which a plugin crate installs under
  `$XDG_DATA_HOME/tau/skills` as its host starts, marked "built in" on
  the Skills screen. The worked plugins are files of their own, inlined
  into `SKILL.md` as it is installed and run as tests of the host, so
  the skill cannot drift from the interface. The skill
  (`crates/plugins/tau-luau-plugins/skill/`) is the interface's
  reference.
- **`/plugin <what it should do>`** in any chat starts a chat in the
  plugins repository whose first message is `/tau-plugins <what it
should do>`, which loads the skill, with the person's words as its
  task. "Edit with tau" on a plugin's page does the same for that
  plugin.
- **Tests** are Luau files under `tests/`, run in fresh VMs with a fake
  run:

  ```luau
  local tau = require("tau")
  local ui = tau.ui
  local t = tau.test   -- the plugin in the folder is the one tested

  t.case("blocks deploys on Friday", function()
    local run = t.run { now = "2026-10-09T10:00:00Z" }   -- a Friday
    local result = run:before_tool { name = "bash", args = { command = "make deploy" } }
    t.equal(result, tau.block("No deploys on Friday."))
  end)

  t.case("the card says when to wait", function()
    local run = t.run { now = "2026-10-09T10:00:00Z" }
    t.equal(run:card("deploy_window", {}, { ok = false }), ui.badge("wait", "warn"))
  end)
  ```

  `t.run` takes a clock, settings, state and fake tool results (as
  codemode's module tests do); every hook can be called on it. Tests
  run on every reload, and with a `plugin_test` tool the plugins
  repository's runs have, so the agent sees failures as it works.

### Order of work

1. The plugins repository, the registry and its reloads, tools,
   `before_tool`, `before_stop`, `turn_end`, `run_end`, state, the
   status line and tool cards, tests and `plugin_test`, the built-in
   skill and `/plugin`. Done; in it, `ctx.tools` is reached from tools'
   handlers only, and settings are the declared defaults.
2. The rest of the view (notes, pages, buttons and actions), settings
   forms, `ctx.tools` in the other hooks, "Edit with tau", pinning and
   rollback, plugins from other repositories.
3. Perhaps more seams: lines in the first message, then results.

## Alternatives considered

- **Plugins as codemode modules only.** Already content-addressed and
  tested, but edited through `module_define` calls rather than files: a
  long plugin, its tests and README are far easier to read, diff and
  revise as a folder in a repository.
- **A folder outside any repository.** Simpler, but no history, no
  diffs, no landing, and the agent's file tools could not reach it.
- **Approve every version by hand.** Safe, but rewriting a plugin ten
  times would mean ten approvals of changes the person just asked for.
- **Activate every version by itself.** No friction, but a run's model
  could widen what a plugin reaches without the person noticing.
- **A Luau binding to gpui.** Arbitrary UI, but it would not run on the
  phone and would tie every plugin to gpui's API.
- **WebAssembly in place of Luau.** Any language, but a second sandbox
  beside codemode's, with none of its tests or promotion.

## Consequences

- Making or changing a plugin is a conversation: no Rust, no rebuild,
  and the interface shows each version as it lands.
- tau owns one local repository, an exception to "repositories come
  from GitHub" (0015). It does not push anywhere unless the person adds
  a remote.
- Every hook pays for a VM, a few milliseconds; per tool call that is
  fine, and `turn_end` is once a turn.
- The view vocabulary and the Luau interface are interfaces of their
  own: changing them touches the renderer, the phone, the skill and its
  tests.
- Codemode's sandbox, module tests and promotion serve two users;
  changes to them are checked against both.
