# 0027: Luau plugins, hosted by one Rust plugin

- Status: proposed. Builds on [0005](0005-plugins.md),
  [0017](0017-plugins-bring-their-ui.md) and
  [0018](0018-codemode-and-mcp.md); does not change them for Rust
  plugins.
- Date: 2026-10-05

## Context

Every plugin today is a Rust crate compiled into tau. Someone who wants
a rule ("no deploys on Friday"), a tool for their stack, or a check
before a run stops has to fork tau, write Rust and rebuild.

tau-codemode already runs Luau safely: a fresh VM per call, a memory
cap, a deadline and cancel interrupt, no files, processes or network,
and globals (`tools.*`, `jev.*`, `infer`, the store) that go back
through the run's loop, so rules and other plugins still see each
call. Its modules are content-addressed, pinned by digest, tested, and
promoted into a repository only with the person's approval
([module promotion](../reference/codemode-module-promotion.md)).

Two things stand in the way of reusing that for plugins:

- **The UI.** 0017 has every plugin bring its UI as arbitrary gpui.
  Luau cannot draw gpui, and the phone builds codemode without its
  host half, so it cannot run Luau at all.
- **Trust.** Runs work in a clone the model edits. A plugin loaded from
  the repository's files would let a run's model write code that later
  runs hook: block the person's rules, or read every tool result.

## Decision

### One host plugin

A new crate, `tau-luau-plugins`, is one `UiPlugin` like any other. For
each run it loads the approved Luau plugins and gives each its own
`PluginRun`. Every hook runs the plugin's code in a fresh codemode VM,
with codemode's limits and globals. tau-agent, the delta rule and 0017
are unchanged: Luau plugins live inside one ordinary Rust plugin.

A Luau plugin is a module whose export names its hooks:

```luau
return {
  name = "no-friday-deploys",
  uses = { tools = { "bash" }, jev = false, infer = false },
  tools = {
    deploy_window = {
      description = "Whether now is a safe time to deploy.",
      parameters = { type = "object", properties = {} },
      call = function(args, run) return { ok = true } end,
    },
  },
  before_tool = function(call, run)
    if call.name == "bash" and call.args.command:find("deploy") then
      return { block = "No deploys on Friday." }
    end
  end,
  before_stop = function(answer, run)
    if not answer.text:find("tests") then
      return { continue = "Say which tests you ran." }
    end
  end,
  view = function(state) return { status = { text = "on" } } end,
}
```

### The first seams

1. **Tools**, with a JSON schema and a Luau handler. Every run declares
   them the same way, refusing where they do not apply, so tool parity
   and the shared prompt cache hold
   ([0022](0022-the-prompt-cache-follows-the-connection.md)).
2. **`before_tool`**: allow, block with a reason, or flag for review,
   as tau-constitution does.
3. **`before_stop`**: stop, or go on with a message, as tau-goal does.
4. **`on_event` and `finish`**, to observe and publish records. Events
   reach Luau batched once a turn, not once a streamed delta.

`start` (lines in the first message), `after_tool_result` and
`rewrite_context` come later, if at all: the last two change what the
model saw, where a mistake costs the most. Model and effort stay
Rust-only.

### A declarative UI

A plugin's `view` returns a JSON tree of tau-ui-kit's pieces: text,
rich text, mono, badges, key and value rows, lists, progress, code,
and buttons that send an action. The host calls it and keeps the tree
in the plugin's run state (0017), and one Rust renderer draws it on
the computer and on the phone, which never runs Luau. A tree can fill:

- the card of the plugin's own tools;
- a note in the transcript, and a status line;
- its entry on the Plugins screen, with a settings form drawn from a
  JSON schema;
- later, a page of its own.

A button's action goes back to the host, which runs the plugin's
handler for it, as `act` does for Rust plugins. This is the one place
a Luau plugin gets less than a Rust one: a fixed set of pieces in
place of arbitrary gpui.

### Approved by digest

- A plugin runs only at a version the person approved, by its digest.
  A changed plugin is a new version waiting for approval.
- It declares what it uses: the tools it calls, Jev, `infer`, and its
  seams. Approval shows them, and the host refuses anything else. Jev
  and `infer` are charged to the run, as every plugin's model calls
  are.
- **User plugins** live in `~/.config/tau/plugins` and are approved
  once.
- **Repository plugins** arrive through codemode's promotion, with its
  receipts. One a run's model writes reaches later runs only once the
  person approves it.

### Order of work

1. The host plugin, tools, `before_tool`, `before_stop`, events, a
   status line and its tools' cards; user plugins only.
2. The full declarative UI (notes, pages, settings, actions),
   repository plugins through promotion, and a flow where the model
   writes a plugin and its tests.
3. `start`, and perhaps `after_tool_result`.

## Alternatives considered

- **Load plugins from the repository's files.** The simplest to share,
  but a run's model could edit what later runs execute.
- **User plugins only.** Safe and simple, but a team could not share
  plugins through the repository, and codemode's promotion already
  makes that safe.
- **Arbitrary UI through a Luau binding to gpui.** It would not run on
  the phone, and it would tie every Luau plugin to gpui's API.
- **Headless Luau plugins.** Contradicts 0017: a plugin without a UI
  cannot be seen or controlled.
- **WebAssembly in place of Luau.** Any language, but a second sandbox
  beside codemode's, with none of its modules, tests or promotion.

## Consequences

- Writing a rule, a tool or a stop check no longer needs Rust or a
  rebuild.
- Every hook pays for a VM, a few milliseconds; per tool call that is
  fine, and events are batched per turn.
- The UI vocabulary becomes an interface of its own: adding a piece
  touches the renderer and the phone.
- Codemode's module and promotion code serves two users; changes to it
  are checked against both.
