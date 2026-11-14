# MCP source playbook

Use this for any MCP server in the run that holds tickets or issues, long-form docs, team chat, infrastructure observability, error tracking, or product analytics. Its tools appear as `mcp__<server>__<tool>`. Use only read and search tools. Never call a tool that writes, posts, edits or resolves anything.

## Before you search

1. Read the server's instructions and its tools' descriptions. Learn what it can search, how it filters by date, and what it returns.
2. Make one small call to confirm it works. If it fails or needs authentication you do not have, stop and report the gap.

## How to search

1. **Start from the anchor.** Fetch every item the anchor names first: ticket ids, PR numbers, URLs, error strings, metric or table names in the code.
2. **Search by the target's words.** The feature name, the key symbols, the constant's value, the exact error message. Try several phrasings.
3. **Bound by time and author.** Search around the dates of the commits in the anchor, and for items by the commit authors. This narrows the search more than any keyword.
4. **Walk the structure your source has.** A sub-ticket's parent, a doc's linked pages, a chat thread's replies, a dashboard's monitors. Parents and threads often hold the reason the leaf item leaves out.
5. **For numeric sources** (observability, analytics), record the exact query, the time window and the numbers you got. Aggregate in the query instead of reading raw rows.

## Pitfalls by kind of source

- **Tickets:** scope changes over a ticket's life, and template "why" sections filled with boilerplate. Read the whole history.
- **Docs:** specs written before the code and never updated. A doc that says "we will do X" while the code does Y is a contradiction to record.
- **Chat:** a single message without its thread reads differently. Fetch threads. A joke is not a decision. Retention limits cut off old history: name the cliff.
- **Observability and error tracking:** correlation is not causation. A metric existing shows someone cared to measure it, not that the code exists because of it. Telemetry gets renamed or expires: missing data from the window is a gap, not a null result.
- **Analytics:** confirm a table exists and holds the column before you report a result from it. A step change in volume may be new instrumentation, not new behaviour.

## What to return

Use the investigator template's output format. For each item, give its kind (ticket, doc, thread, dashboard, error group, query), title, location or URL, date, and the verbatim text or numbers that bear on the question.
