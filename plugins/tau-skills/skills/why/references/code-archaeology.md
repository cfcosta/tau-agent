# Code archaeology (source control and the repository)

## What this source contains

- Commit history: descriptions, authors, dates, diffs
- Inline comments, TODOs, FIXMEs, deprecation notes, lint suppressions that give a reason
- Decision records (ADRs) and design notes, if the repository keeps them
- Tests, whose names and assertions often encode the edge case that motivated a change
- Files changed in the same commits (co-change signal)
- CHANGELOG entries and release notes
- PR numbers and ticket ids in commit descriptions

## How to search it

tau's version control tools work on jj. Do not run `git` or `jj` through `exec_command`.

- `vcs_log {limit}` lists `@` and its ancestors, newest first, with the first line of each description. Pass a large `limit` to reach old history.
- `vcs_show {change}` gives one commit's full description and its diff. The output is cut at 50KB.
- The log cannot filter by path or by text. Use `codemode` for that: walk the log, call `vcs_show` on each commit in parallel, and return only the commits whose diff touches the target path, or adds or removes the exact string you care about. This stands in for `git log --follow -- <file>` and `git log -S '<string>'`.
- Renames are not followed. If the file or symbol had another name, search the diffs for the old name too. A commit that deletes one path and adds a near-identical one is a rename.
- There is no line-level blame. To date a line, search the diffs for the commit that added its exact text. Say in your report that blame was not available.

Look in the tree as well:

- `grep` for decision records: `architecture decision`, `ADR`, `decisions/`, `docs/`.
- `grep` the target file for `TODO|FIXME|HACK|XXX|NOTE|WARNING`.
- `grep` the tests for the target's symbols. A test added in the same commit as the code is strong evidence of the case the author had in mind.

## What good evidence looks like here

- A commit body that states the reason: "clamp to 100: the upstream API rejects larger pages".
- A test named for the case: `rejects_page_size_over_upstream_limit`.
- A revert followed by a re-apply "with X fixed", which shows what went wrong the first time.
- A comment that names an incident, an issue or a person to ask.

## Common pitfalls

- **Bulk commits.** A formatting pass or a large move is the last touch of many lines but explains none of them. Look past it to the commit before.
- **Squashed history.** A single commit may stand for a long discussion that lives elsewhere. Record the PR or ticket number as a lead.
- **Messages that restate the diff.** "update limit" is mechanics. It is not a reason.
