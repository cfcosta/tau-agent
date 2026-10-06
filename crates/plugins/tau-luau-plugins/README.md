# tau-luau-plugins

The interface half of Luau plugins. A Luau plugin is a folder in tau's
own plugins repository: `plugin.luau`, which returns `tau.plugin { ... }`,
modules under `lib/`, tests under `tests/` and a README. tau runs all of
them as one tau plugin, each hook in a fresh codemode VM. This crate
holds what every interface shows of them: what a plugin declares, where
it stands, the records a run keeps, and the views drawn from them. It
runs no Luau, so the phone draws what the computer does.

## What it provides

| Item                            | What it is                                                  |
| ------------------------------- | ----------------------------------------------------------- |
| `LuauPluginsUi`                 | The `UiPlugin`: the Plugins page, status line, tool cards   |
| `Declaration`                   | What a plugin declares, read without running its hooks      |
| `Uses`, `ToolSpec`, `Hooks`     | What it may reach, the tools it adds, the hooks it has      |
| `Declaration::grown_from`       | What a new version reaches that the allowed one did not     |
| `Standing`, `Entry`, `Overview` | Where each plugin stands at trunk                           |
| `TestResult`                    | One case of a plugin's tests                                |
| `Record`                        | What the host publishes about a run's plugins               |
| `Act`, `SettingsPage`           | What the page asks the host, and a drawn settings page      |
| `LuauSettings`                  | The person's settings for each plugin                       |
| `settings`                      | Settings schemas: `check`, `effective`, `fields`            |
| `pane`                          | A plugin's settings pane: its own page, or a form           |
| `repository`                    | The plugins repository's directory (`ROOT`) and first files |
| `TAU_MODULE`                    | The `tau` Luau module every plugin requires                 |
| `NAME`, `REPO`, `SKILL`         | `tau-luau-plugins`, `tau-plugins`, and the skill's name     |

A plugin's `view`, `settings.view` and tool cards return JSON view
trees. `ui::draw` turns them into tau-ui-kit pieces, so drawing needs no
Luau.

## How it fits

This is the interface half (decisions 0017, 0027 and 0030). Its partner
is `tau-luau-plugins-host`, which reads the plugins repository, loads
each plugin, runs its hooks and publishes the `Record`s this crate
folds.

It builds on `tau-ui-plugin` and `tau-ui-kit`, and draws with gpui. It
does not depend on `tau-codemode` or `tau-agent`.

Used by:

- `tau-luau-plugins-host`, the host half;
- `tau-ui-remote`, which registers `LuauPluginsUi` after the
  repository's rules and the goal, so theirs hold first;
- `tau-ui`, which lists the plugins repository (`REPO`) and creates it
  under `repository::ROOT` with `repository::first_files`.

## Usage

A plugin's `plugin.luau`, shortened from the skill's worked example in
`tau-luau-plugins-host/skill/examples/no-friday-deploys`:

```lua
local tau = require("tau")

return tau.plugin {
	name = "no-friday-deploys",
	description = "Blocks deploy commands on Fridays.",
	settings = { default = { days = { "Friday" } } },
	before_tool = function(call, ctx)
		local command = call.args.command
		if call.name == "bash" and type(command) == "string" and command:find("deploy") then
			if tau.contains(ctx.settings.days, ctx.now.weekday) then
				return tau.block("No deploys on " .. ctx.now.weekday .. ".")
			end
		end
	end,
}
```

Its `Declaration` has `hooks.before_tool` set. A hook that can block a
call is new reach, so its first version waits on the Plugins screen
until the person allows it.

## Testing

```sh
cargo nextest run --release -p tau-luau-plugins
```

The tests are unit tests in `src/`: what counts as growth between
versions, settings checked against their schema and drawn as a form,
and records folded by plugin. Loading and running plugins is tested in
`tau-luau-plugins-host`.

## Further reading

- [0027: Luau plugins, written and rewritten through tau](../../../docs/decisions/0027-luau-plugins.md)
- [0029: Every plugin's settings in a pane of their own](../../../docs/decisions/0029-every-plugins-settings-in-a-pane.md)
- [0017: Plugins bring their own UI](../../../docs/decisions/0017-plugins-bring-their-ui.md)
- [0030: Host halves are crates](../../../docs/decisions/0030-host-halves-are-crates.md)
- [Codemode reference](../../../docs/reference/codemode.md), for the
  sandbox the hooks run in
