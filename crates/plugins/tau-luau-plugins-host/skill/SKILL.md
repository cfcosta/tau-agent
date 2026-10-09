---
name: tau-plugins
description: Writing, changing and testing tau's Luau plugins, in the tau-plugins repository. Load it before you write or change a plugin, or when someone asks tau to do something on its own whenever a run calls a tool, stops, or ends a turn.
---

# tau's Luau plugins

A plugin changes how tau's runs go: it adds tools the model can call,
looks at each tool call before it runs (to block it or flag it for
review), checks a run before it stops, and shows a line in the run's
status. Plugins are Luau files in the `tau-plugins` repository, one
folder each:

```
<name>/
  plugin.luau      -- returns the plugin: tau.plugin { ... }
  lib/<module>.luau  -- modules of its own: require("<module>")
  tests/<file>.luau  -- its tests, run against fake runs
  README.md        -- what it is for, for the person
```

The folder's name is the plugin's `name`. A plugin is active once its
commit is on `main` and its tests pass. Write it, write its tests, run
`plugin_test` with the plugin's name until they pass, then commit. When
your turn ends, tau lands your commits on `main` by itself if every
plugin you changed passes and reaches nothing new; this chat goes on.

## What the person sees

When `main` moves, tau loads every plugin, runs its tests and shows the
outcome on the Plugins screen:

- **Active**: its tests pass, and it reaches nothing it did not before.
- **Waiting**: it reaches further than the version the person allowed:
  a new tool in `uses`, `jev` or `infer`, or a new `before_tool` or
  `before_stop`. The person clicks Allow; until then, the version
  before stays active. Say so when you commit such a change.
- **Failing**: its tests fail; the version before stays active.
- **Broken**: it does not load; the error and its line show.

A change is live at once, with no new chat or message:

- **In this chat**, a plugin as this workspace has it runs from your
  next model request, once `plugin_test` would pass and it reaches
  nothing new. Write it, test it, then use it right here.
- **Everywhere else**, once it lands on `main`: tau lands this chat's
  commits when a turn ends with every plugin you changed passing and
  reaching nothing new, and this chat goes on from there. Work that
  fails its tests, waits for the person, or would conflict stays here
  until a later turn fixes it; say so to the person.
- A tool the run did not start with is not in your tool list: call it
  from `codemode`, where `search_tools` finds it. From the next message
  it is a tool like the others. A changed tool you already have runs
  its new version.

## The plugin

```luau
local tau = require("tau")
local ui = tau.ui

return tau.plugin {
  name = "...",              -- the folder's name
  description = "...",       -- one line, for the Plugins screen
  uses = { tools = { "bash" }, jev = false, infer = false },
  settings = { schema = { ... }, default = { ... }, view = function(settings, ctx) end },
  tools = { name = { description, parameters, call, card } },
  before_tool = function(call, ctx) end,
  before_stop = function(stop, ctx) end,
  turn_end = function(turn, ctx) end,
  run_end = function(run, ctx) end,
  view = function(state, ctx) end,
}
```

Every field but `name` is optional.

### Hooks

| Hook          | Given                                      | Returns                                    | Time   |
| ------------- | ------------------------------------------ | ------------------------------------------ | ------ |
| tool `call`   | the arguments, as `parameters` describes   | a JSON value, or `error(message)`          | 60 s   |
| tool `card`   | the call's arguments and the result        | a view piece, drawn on the call's card     | 200 ms |
| `before_tool` | `{ id, name, args }`                       | `nil`, `tau.block(why)` or `tau.flag(why)` | 2 s    |
| `before_stop` | `{ text, turn, reason }`                   | `nil`, or `tau.continue(message)`          | 10 s   |
| `turn_end`    | `{ turn, text, calls = { { name, ok } } }` | nothing                                    | 2 s    |
| `run_end`     | `{ stop, turns, cost }`                    | nothing                                    | 10 s   |
| `view`        | the run's state                            | `{ status = ui.status(...) }`, or `{}`     | 200 ms |

- `tau.block(why)` keeps the call from running; the model reads why.
  `tau.flag(why)` lets it run and marks it for the person's review.
- `tau.continue(message)` keeps the run going with `message` as the
  next thing the model reads; a run continues at most three times.
- A hook that errors or runs out of time counts as `nil`, and its error
  shows on the Plugins screen. After three failures in a run the plugin
  is off for the rest of it. Never rely on an error to block.
- Hooks run each in a fresh VM: keep what must last in `ctx.state`, not
  in local variables.

### `ctx`

| Field          | What it is                                                                                        |
| -------------- | ------------------------------------------------------------------------------------------------- |
| `ctx.run`      | `{ id, kind = "main" or "chat" or "sub_agent", repo, model, turn }`                               |
| `ctx.now`      | `{ unix, iso, weekday }`; `weekday` is `"Monday"` and so on                                       |
| `ctx.settings` | what the person set, over `settings.default`; per repository when one keeps its own copy          |
| `ctx.state`    | the plugin's table for this run; what a hook leaves in it is kept                                 |
| `ctx.tools`    | in a tool's `call` only: the tools `uses.tools` names, called as `ctx.tools.read({ path = "a" })` |
| `ctx.jev`      | Jev, when `uses.jev`                                                                              |
| `ctx.log`      | `ctx.log(text)`: a line on the plugin's page, never for the model                                 |

`ctx.state` is stored with the run: a resumed or forked run keeps it.
JSON has no empty array: use `array({})` for one that must stay a list.

### `tau`

- `tau.plugin(table)`, `tau.block(why)`, `tau.flag(why)`,
  `tau.continue(message)`.
- `tau.contains(list, value)`: whether `list` holds `value`.

### `tau.ui`: what views are made of

Plain tables, drawn by tau on the computer and on the phone:

- `ui.text(text)`, `ui.rich(text)` (with `` `code` `` and `**bold**`),
  `ui.mono(text)`
- `ui.badge(text, tone)`; a tone is `"neutral"`, `"good"`, `"warn"`,
  `"bad"` or `"info"`
- `ui.rows({ { "key", "value" }, ... })`, or `ui.rows({ key = value })`
- `ui.list({ ... })`, `ui.stack({ ... })` (one under another),
  `ui.row({ ... })` (side by side); strings in them are text
- `ui.progress(share, label)` with `share` from 0 to 1
- `ui.code(lang, text)`
- `ui.status(text, detail, tone)`: the run's status line, from `view`

`view` is called after each hook that changed the state. It must only
read the state: it has no tools.

### Settings

What the person can set lives in `settings`:

- `schema`: a JSON schema of an object, its properties each a boolean,
  a string (with an `enum`, a choice), a number, an integer, or an
  array of strings with an `enum` (a choice of several). Nothing is
  saved that it does not accept, and it takes no keys beyond its
  properties.
- `default`: the values before anything is set.
- `view(settings, ctx)`, optional: the plugin's own settings page,
  drawn from `tau.ui`. Without it, tau draws a form from `schema`.

The page is drawn in the plugin's pane on the Plugins screen, and in a
chat's side panel. Three pieces are bound to a key of the settings
(dotted for a table inside: `limits.max`), and changing one saves it:

- `ui.toggle(key, label)`: a switch, for a boolean.
- `ui.choice(key, options, { multi = true })`: one of `options`, or
  several.
- `ui.field(key, { kind = "number", placeholder = "..." })`: a line of
  text, or a number; Enter saves it.

The person sets them everywhere or per repository. A value the schema
no longer accepts gives way to the defaults, and the pane says so.

### Modules

`lib/<module>.luau` is `require("<module>")` in `plugin.luau`. A module
may require `tau`, but not another module of the plugin. `tau` and
`plugin` are taken.

## Tests

Each file under `tests/` is a list of cases; `plugin_test` and every
reload run them all. `t.run(spec)` is a fake run of the plugin, and its
hooks are methods on it:

```luau
local tau = require("tau")
local t = tau.test

t.case("what it checks", function()
  local run = t.run {
    now = "2026-10-09T10:00:00Z",  -- an ISO date: a Friday
    settings = { ... },            -- else the defaults
    state = { ... },               -- else {}
    tools = { read = "text" },     -- what ctx.tools give: a value, or a function of the args
  }
  t.equal(run:before_tool { name = "bash", args = { command = "ls" } }, nil)
end)
```

- `run:tool(name, args)`, `run:card(name, args, result)`,
  `run:settings_view(settings)`,
  `run:before_tool(call)`, `run:before_stop(stop)`,
  `run:turn_end(turn)`, `run:run_end(run)`, `run:view()`.
- `run.state` is the state the last hook left; `run.logs` its log lines.
- `t.equal(got, want, what)` compares tables by what they hold.

Test what the plugin decides, on the days, commands and states where
its answer changes, and its card and status line.

## Three plugins

### A rule: `before_tool`

{{no-friday-deploys}}

### A tool with a card, a module and a status line

{{word-count}}

### A stop check: `turn_end` and `before_stop`

{{say-the-tests}}
