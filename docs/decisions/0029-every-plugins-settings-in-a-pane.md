# 0029: Every plugin's settings in a pane of their own

- Status: accepted. Amends [0017](0017-plugins-bring-their-ui.md): a
  plugin's settings are drawn in a pane the host places, not inside its
  page. Amends [0027](0027-luau-plugins.md): Luau plugins' settings
  forms and pages come now, and each Luau plugin is listed on its own.
- Date: 2026-10-05

## Context

Settings are wherever each plugin put them. tau-reasoning draws its two
controls inside its page, tau-mcp edits servers on its page,
tau-direnv's per-repository switch lives in a repository menu, and most
plugins have no settings at all. The Plugins screen is a flat list in
two columns: name, description, and either the open run's state or what
the plugin cost. A Luau plugin's settings are its declared defaults, and
all Luau plugins share one row.

So there is no one place to answer "how is this plugin set, and where
do I change it", and settings are global: a repository cannot set a
plugin differently from the rest.

## Decision

### The Plugins screen: a list, and a pane

- **The list** on the left groups plugins by what they are for:
  Context, Rules, Tools, Environment, and Yours (Luau plugins). A
  plugin says its group in its catalog entry (`PluginInfo::group`).
  Each row shows a short note (a count, "waits for you", "tests
  fail"), and a filter matches names, descriptions and setting names.
- **The pane** on the right shows the selected plugin under three tabs:
  - **Settings**: the plugin's settings pane, with the scope switch
    (below) and a Reset when they differ from the defaults. A plugin
    without settings says so.
  - **What it did**: the plugin's page, as today.
  - **How it works**: its description, where it steps into a run (its
    seams), what it reaches, and for a Luau plugin its README, tests
    and version.
- A plugin can list **entries of its own** (`PluginInfo::entries`):
  tau-luau-plugins lists each Luau plugin, which is then a row like any
  other, selected as `<plugin>/<entry>`.

### Every plugin draws its settings in its own pane

`Manifest::settings` registers a function that draws the plugin's
settings, given its value for the scope shown and a handle that saves a
new one. The host places the pane on the Plugins screen and in the
inspector; a page no longer draws settings. tau-reasoning, tau-mcp and
tau-direnv move theirs there.

### Scopes: everywhere, or one repository

- A plugin's settings are kept **everywhere**, and a repository may keep
  **its own copy**, which wins in that repository's runs. A copy is a
  whole value, not a patch: the pane starts it from the value
  everywhere and the person changes what differs.
- The scope switch shows Everywhere and the repositories; a repository
  with its own copy is marked, and "Use the value everywhere" drops the
  copy.
- Runs take the settings as they start, as today: a change applies from
  each chat's next message.
- Per chat is left for later: it needs settings stored with the run and
  kept when it resumes or forks.

### The inspector opens the same pane

The inspector's Plugins section lists what each plugin did in the run.
A row opens that plugin's settings pane in place, scoped to the run's
repository, with a link to the Plugins screen.

### Luau plugins: a form from the schema, or a page of their own

```luau
settings = {
  schema = { type = "object", properties = {
    days = { type = "array", items = { type = "string",
      enum = { "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday" } } },
  } },
  default = { days = { "Friday" } },
  -- Optional: the plugin's own settings page.
  view = function(settings, ctx)
    return ui.stack {
      ui.rich("Deploys are blocked on these days."),
      ui.choice("days", { "Monday", "Tuesday", "Wednesday", "Thursday", "Friday" }, { multi = true }),
    }
  end,
}
```

- **Without `view`**, the host draws a form from `schema`: a boolean is
  a switch, a string with `enum` a choice, an array of such strings a
  choice of several, a string or number a field. A property of any
  other shape is listed read-only with its value.
- **With `view`**, the plugin draws its own page from `tau.ui`, plus
  three pieces bound to a key of its settings: `ui.toggle(key, label)`,
  `ui.choice(key, options, { multi })` and `ui.field(key, { kind,
  placeholder })`. Changing one changes that key; the host checks the
  value against `schema` before saving it, and the page says why when
  it does not hold. `view` runs on the host in its own VM within 200
  ms, again after each change, and its tree is sent to the interface,
  so the phone draws it too.
- **Saved values** are kept by tau-luau-plugins as its own settings,
  one value per Luau plugin, so the scopes above apply to them. A run's
  `ctx.settings` is the saved value for its repository, filled in from
  `default`; a value the schema no longer accepts falls back to the
  default and the pane says so.

## Consequences

- One place for every setting, on the computer and on the phone.
- A repository can set a plugin differently: a stricter rule, an MCP
  server only there.
- The Plugins screen needs the catalog's settings for every repository,
  not only the global ones.
- A Luau plugin's settings page is the view vocabulary's first input:
  bound pieces need care so a page cannot write keys its schema does
  not have.
