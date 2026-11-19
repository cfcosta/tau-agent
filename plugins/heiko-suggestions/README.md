# heiko-suggestions

Suggests what the person may ask next in a repository, as Codex's
desktop app does with its ambient suggestions (ADR 0063). When a run
ends, `run_end` keeps what it was about in the repository's store: the
person's last messages, the tools called with the start of what they
gave, and the last answer. It asks no model, so the run ends as soon as
it would have. A tick every five seconds takes each repository's request
and asks the plan's newest Luna at low effort for a few suggestions,
each a title, the prompt behind it, and why. They replace the pending
ones.

The newest pending suggestion is the repository's `repo_data.composer`:
the empty composer of a new run there says its title, and Tab writes its
prompt. A run that starts with that prompt took it. The Suggestions page
of a repository lists them, with Put in composer, Start and Dismiss, and
asks for a new set from the last run on Suggest again. What the requests
cost is booked to the plugin.

`src/plugin.luau` has the hooks, the tick, the page and its actions;
`src/suggest.luau` cuts the conversation down, writes the request with
its rules, and keeps the list.

`heiko-plugin test plugins/heiko-suggestions` loads the plugin as heiko
does and runs its Luau tests.
