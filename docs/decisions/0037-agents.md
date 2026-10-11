# 0037: Agents: a conversation that never ends, and rewrites itself

- Status: accepted; the four steps of its order of work are built.
  Where it says `bash`, read `exec_command` and
  `write_stdin` ([0038](0038-commands-run-in-the-background.md)): the
  sandbox is their launcher. Builds on [0018](0018-codemode-and-mcp.md),
  [0025](0025-agent-commands-run-in-an-environment-plugins-give.md),
  [0026](0026-sub-agents-run-detached.md),
  [0027](0027-luau-plugins.md), [0031](0031-the-main-chat-orchestrates.md)
  and [0035](0035-idle-chats-compact-before-their-cache-lapses.md).
  Amends [0015](0015-a-main-chat-per-repository.md): tau keeps one
  local repository per agent, beside the plugins repository. Amends
  0018: tau-mcp subscribes to resources, and an agent's own servers
  connect without the person's approval. Amends
  [0036](0036-the-plugins-repository-is-in-the-sidebar.md): the
  sidebar gains an Agents section of its own.
- Date: 2026-10-11

## Context

Every run in tau starts from a person's message and ends when its turn
does. Some work has no message to start it: a pull request that opens,
a build that fails, a feed that changes, a time of day. The person
wants something that keeps going on its own, reacts to those events,
remembers what it did, and gets better at its job by changing itself.

tau has most of the parts. A repository's main chat is one
conversation that goes on across runs (0015) and compacts as it grows
(0035). Codemode runs Luau that calls tools, keeps modules as tools
and stores large data outside the prompt (0018). Luau plugins showed
that an agent can write, test and land Luau that the host loads at
once (0027, 0034). Sub-agents work in parallel and land through main's
queue (0026, 0031).

What is missing:

- **Nothing wakes a conversation.** tau has no scheduler (0001), no
  way to reach a conversation from outside a run, no network in
  codemode, and tau-mcp does not subscribe to resources (0018 left
  `resources/subscribe` for later).
- **Nothing keeps it to itself.** A main chat's `bash` runs as the
  person, with their files and keys. An agent that acts unattended
  needs a wall, not a convention.
- **Nothing stops it spending.** A conversation that never ends, woken
  by every event, has no natural end to its cost.

## Decision

### The model is the agent

- An agent is a local jj repository under
  `$XDG_DATA_HOME/tau/agents/<name>`. Its **main chat is the agent**:
  one conversation that never ends. Compaction keeps it going, idle
  compaction included (0035).
- **Its instructions are its repository's `AGENTS.md`** (0020). It
  wrote them first, and it may change them.
- It works the way any main chat does: codemode scripts, codemode
  modules as its own tools, the file tools in its repository, a
  sandboxed `bash` (below), and sub-agents that land through its queue
  (0026, 0031). The model decides what each event means and what to do
  about it. Luau declares what wakes it and gives it tools; Luau does
  not decide for it.
- **Its own repository only.** Its tools, its `bash` and its
  sub-agents never reach the person's repositories.
- **Memory** is three things: tau-memory's notes, scoped to its
  repository (`Scopes::repo`, with the person's `Scopes::user` notes
  as for any run); the conversation itself; and the JSON state its
  triggers keep (cursors, seen keys), stored as records of the agent.
- **Model and effort are per agent**, set by the person on its page.
  A new agent starts on the plan's newest small model (the `luna`
  family of `PLAN_FAMILIES`, `gpt-6-luna` today) at low effort. The
  person's own messages in its chats go to them too, unless they pick
  others in the composer (`HostHalf::conversation_model`).

### Events are messages in its conversation

- Every event goes into a queue the host stores, and reaches the model
  as a message in its conversation. An event waits a coalescing window
  (30 s by default, per agent) so a burst arrives as one message.
  Events that come while a turn runs wait for the next turn and arrive
  together.
- The message is tau's, not the person's, and reads:

  ```text
  [events · 3 · 14:27:03–14:27:31]
  - pull_requests · poll · pull_requests:412 · 14:27:03
    {"number":412,"title":"Plugins sidebar follow-up","author":"..."}
  - builds · stream · build:2318 · 14:27:20
    {"number":2318,"state":"failed","commit":"7c7845bd"}
  - pr-triage · message · msg:9f2c · 14:27:31
    "#412 touches tau-store; you own that."
  ```

  Each event is its trigger's name, its kind, its id, when it came, and
  its data as JSON. Data over 8 KiB is stored as a codemode artifact
  (0018, M4) and the line holds its reference, so a large payload never
  fills the conversation.

- **At least once.** An event leaves the queue when the turn it is in
  has started and its message is stored. tau closed in between delivers
  it again. Ids let the model, and the trigger's dedupe, see a repeat.

### `agent.luau` declares what wakes it

`agent.luau`, at the root of its repository, returns a table. The host
loads it in a fresh codemode VM each time trunk moves.

```luau
local tau = require("tau")

return tau.agent {
  name = "pr-triage",

  triggers = {
    -- New open pull requests, from the GitHub MCP server.
    pull_requests = {
      poll = { tool = "mcp__github__list_pull_requests",
               args = { owner = "cfcosta", repo = "tau-agent", state = "open" } },
      every = "10m",
      key = function(pr) return pr.number end,
    },

    -- An HTTP endpoint, polled, with a key from tau's secret store.
    releases = {
      poll = { http = {
        url = "https://api.github.com/repos/cfcosta/tau-agent/releases",
        headers = { authorization = tau.secret("github") },
      } },
      every = "1h",
      key = function(release) return release.id end,
    },

    -- A stream it keeps open. `parse` frames; it does not judge.
    builds = {
      stream = { sse = "https://ci.example.com/v1/pipelines/tau/events",
                 headers = { authorization = tau.secret("ci") } },
      parse = function(frame, state)
        if frame.event == "ping" then return nil end
        local build = json.decode(frame.data)
        return { id = "build:" .. build.number, data = build }
      end,
    },

    -- A server's resource, through MCP's resources/subscribe.
    ci_status = { mcp = { server = "ci", uri = "ci://tau-agent/main" } },

    evening = { at = "18:00", days = { "Mon", "Tue", "Wed", "Thu", "Fri" } },
    triage = { command = "pr-triage", hint = "<number>" },
    inbox = { message = true },
  },
}
```

| Trigger  | Declared as                                         | Fires                                                                                   |
| -------- | --------------------------------------------------- | --------------------------------------------------------------------------------------- |
| schedule | `{ every = "15m" }` or `{ at = "09:00", days? }`    | at each interval, or at the host's local time; `data` is `{ due }`                      |
| poll     | `{ poll = { tool, args } \| { http }, every, key }` | once per item whose `key` it has not seen; the first call only records keys             |
| stream   | `{ stream = { sse \| websocket, headers }, parse }` | once per frame `parse` turns into `{ id, data }`; `nil` is a frame that is not an event |
| mcp      | `{ mcp = { server, uri } }`                         | when the server sends `notifications/resources/updated` for `uri`; `data` is the read   |
| command  | `{ command = "name", hint? }`                       | when the person sends `/name args` in any composer; `data` is `{ args, from }`          |
| message  | `{ message = true }`                                | when another agent sends it a message (below)                                           |

- **`parse` frames; it does not judge.** It reads one frame and the
  stream's state, and says what event, if any, the frame is: a
  keep-alive is `nil`, a payload is an event. Deciding whether an event
  matters is the model's.
- **Polls** keep the last 10 000 keys in the agent's state. A poll's
  result must be a list, or `items = function(result)` picks it. A
  `tool` poll calls a tool outside any run, through tools plugins offer
  the host for that (`OutsideTools`): today an MCP server's, whose
  result is its structured content, else its text read as JSON. An
  `http` poll's result is the answer read as JSON, else its text; one
  that is not a success fires nothing. A poll that fails shows why on
  its trigger, and after three in a row waits four intervals, at most
  an hour.
- **Streams** reconnect with back-off and send `Last-Event-ID` when the
  server gave one. The host holds the connection; Luau never does.
- **MCP subscriptions** are new to tau-mcp: it subscribes while an
  active agent names the resource and the server can
  (`resources.subscribe`), on 2026-07-28 through a
  `subscriptions/listen` of the resource's own, before through
  `resources/subscribe`; agents that watch one resource share the
  subscription. Each change the server tells of is read, and fires
  when what it reads differs from the last read: the event's id is the
  URI and a digest of the read, its data the read. The first read only
  records, as a poll's first call does. A server that cannot is shown
  on the page; a poll is the way round it.
- **Commands** reach every composer through `Manifest::listed_commands`,
  as tau-mcp's prompts do. Sending one starts no run where it was
  typed: the composer says "sent to pr-triage" and the event goes to
  the agent.
- **Tests** are Luau files under `tests/`, run on every reload in fresh
  VMs with fake frames, results and clock:

  ```luau
  local tau = require("tau")
  local t = tau.test

  t.case("a ping is not an event", function()
    local agent = t.agent {}
    t.equal(agent:parse("builds", { event = "ping", data = "" }), nil)
  end)

  t.case("a build frame is an event keyed by its number", function()
    local agent = t.agent {}
    local event = agent:parse("builds", { event = "build", data = '{"number":7,"state":"failed"}' })
    t.equal(event.id, "build:7")
  end)

  t.case("a poll fires only for keys it has not seen", function()
    local agent = t.agent { state = { pull_requests = { seen = { 412 } } } }
    local events = agent:poll("pull_requests", { { number = 412 }, { number = 413 } })
    t.equal(#events, 1)
  end)
  ```

### It rewrites itself, and nothing waits for the person

- **It commits straight to its own trunk** from its conversation, as a
  main chat does; its sub-agents land there through its queue. Each
  time trunk moves, the host reloads `agent.luau`, runs its tests, and
  a version whose tests pass is live at once: triggers start, stop or
  change.
- **A version that fails** leaves the previous one live, and the
  failure (the test, or the Luau error and line) comes back into its
  conversation as an event, so it can fix it.
- **No Allow.** Unlike a plugin (0027), a new trigger, server or tool
  needs no click. Its history is its jj log; the page lists its
  versions, and rolling back is a commit like any other. It may act for
  the person outward (post, comment, send) without asking.
- **Its tools are its codemode modules**, kept in its repository
  (codemode's repository modules). In an agent's repository they
  publish without the person's approval, which codemode's promotion
  asks for elsewhere.
- **Its MCP servers** are its repository's `.tau/mcp.json`. They
  connect without the person's approval, which 0018 asks for in other
  repositories, and their stdio servers start inside its sandbox
  (0025's launcher, below).

### The one hard limit: a daily spend cap

- The cap is the person's, set on the agent's page and kept in tau's
  data, outside the repository. The agent cannot raise it. It starts at
  one dollar a day.
- Everything the agent does counts: its turns, its compactions, its
  sub-agents and their turns, and codemode's `infer`.
- At the cap it takes no more turns until the next day (the host's
  local midnight). Events keep queueing; its row shows "needs you",
  and the person is told as for `ask` (below). The page offers to allow
  more for today.

### `bash` runs in a sandbox

- `bash` in an agent's runs starts through a launcher (0025) that the
  agents' host half gives its repositories: `bwrap` (bubblewrap) with
  the run's workspace read-write, `/nix`, `/etc` and the system's other
  read-only paths, a private `/tmp`, a home of the agent's own (kept
  under tau's data, so what it installs or caches stays), and the
  network. The person's home, other repositories, `/run/user` and
  tau's own data are not there, nor tau's environment variables but
  the few a command needs (`PATH`, locale, terminal, certificates).
  Reference: [environment.md](../reference/environment.md).
- It is 0025's extension point, used for a wall rather than an
  environment: the launcher adds words before the program, as
  tau-direnv's does. An agent's repository takes tau-agents' launcher
  alone, so nothing of another plugin's (direnv's `.envrc`) runs
  outside it. Without `bwrap` on `PATH`, an agent's runs have no `bash`,
  and its page says so; it never falls back to running unsandboxed.
- The agent's stdio MCP servers start through the same launcher, for a
  run or for a poll or `mcp` trigger alike; without `bwrap` they do not
  start, and the server says why.

### Codemode gains `http` and secrets by name

- In an agent's runs, codemode has
  `http.request { method, url, headers, body, timeout }`, which returns
  `{ status, headers, body, json, artifact, truncated }`. It calls
  `http`, a nested tool of the coding tools that tau-ui adds to an
  agent's runs only, so a request goes through the loop like any call.
  A body over 64 KiB comes back as an artifact reference (0018, M4).
  Other runs do not get `http`. An agent's polls do not run it in
  Luau: the host makes the request its trigger declares
  (`tau_tools_host::http::fetch`).
- **Secrets by name.** Each agent has its own store,
  `~/.config/tau/secrets/<agent>.json` (mode 0600, like
  `mcp-auth.json`), filled from the agent's page: its requests reach
  its own keys and no other agent's. `http.secret("github")` in a script and
  `tau.secret("github")` in `agent.luau` are a marker,
  `{ ["$secret"] = "github" }`, that only a request's headers and body
  accept; `"Bearer " .. http.secret("github")` puts text before the
  value. The host puts the real value in as the request leaves, and
  redirects are not followed. Responses and errors are scrubbed of
  every secret's value. The raw value never reaches Luau or the
  conversation.
- The sandbox does not see the secrets file.

### Agents talk to each other

- `agent_send { to, text }` puts a `message` event in another agent's
  queue. It arrives with the others, under that agent's coalescing
  window and cap.
- **A guard against ping-pong.** A message carries a hop count: a
  message sent in a turn that only messages woke is one hop further
  than the furthest of them; a message that does not say how far it
  went counts as far as any. Past 8 hops it is refused, and the sender
  is told why. The turn reads its hops from the event message that
  started it, so a turn the person or anything else started begins a
  chain afresh. An agent cannot message itself, and one whose
  `agent.luau` declares no message trigger refuses messages. The cap alone would let two agents spend both their
  days in minutes and then sit paused; the hop count stops that loop
  without limiting agents that are woken by anything else.

### It reaches the person

- `notify { text }` leaves the person a note, kept on the agent's page
  until they dismiss it (the latest 20). `ask { question, choices? }`
  leaves a question, kept until they answer it, with its choices as
  buttons or an answer field when it offers none. It does not hold the
  turn open as tau-ask's does (0019): the answer comes back as an event
  of its own, kind `answer` from the trigger `ask`, with the question
  and the answer. tau-ask is left out of an agent's runs, so `ask` is
  always tau-agents'. These tools, and `agent_send`, are in an agent's
  runs and no other.
- Both, and the spend cap, reach the person three ways: a "needs you"
  pill on its sidebar row and an item on the Agents page; a desktop
  notification (`tau-ui`'s `notify`, a new kind beside `Asks`); and the
  paired phone, which shows it as an Android notification. tau-remote
  has no push service, so the phone hears while it is connected; a push
  for when its app is closed is new work, left for later. The desktop
  and the phone decide when the same way (`tau_agents::alert::Watch`):
  once for each note or question, once each time an agent reaches its
  cap, and never for what was there when they first saw the agent.

### Creating one, and where it shows

- **`/agent <purpose>`** from any composer makes its repository and
  sends the person's words as its first message, with the built-in
  `tau-agents` skill (as `tau-plugins` is in 0027): the triggers, `http`,
  secrets, tests, and that it writes its own `AGENTS.md` and
  `agent.luau`.
- **New agent** on the Agents page is a short form: name, purpose,
  model, daily cap, and which of the person's MCP servers it starts
  with. It ends the same way.
- **The sidebar** has an Agents section of its own, between
  Repositories and Everywhere: one row per agent with its state
  (working, waiting, paused, needs you, broken), its last event, and
  its sub-agents at work folded under it.
- **The agent's page is its conversation.** Event messages draw as
  compact cards in it. A side panel holds what wakes it (each trigger,
  its last and next firing), its spend against its cap, its model,
  its memory, its state, its versions of `agent.luau` with their
  tests, and its secrets by name.

### Agents run while tau runs

There is no daemon yet (0013 left `tau-host` for later). On start,
each agent catches up from what it stored:

- a schedule that came due while tau was closed fires once, with the
  first missed time as `due`, however many it missed;
- every poll runs at once;
- streams reconnect, with `Last-Event-ID` where the server gave one;
- MCP subscriptions subscribe again and read each resource once,
  firing if it changed since the last read;
- commands and messages have nothing to catch up on.

### Order of work

1. Agent repositories and their registry, the conversation, the stored
   queue and coalescing, the event message, schedule and command
   triggers, catching up, model per agent, the spend cap, the sidebar
   section, the page, `/agent`, the form and the skill. No `bash` yet.
2. The `bwrap` launcher and `bash`, `http` and secrets, poll triggers.
3. Stream triggers, `notify` and `ask` with desktop and phone
   notifications, `agent_send` and the message trigger.
4. MCP resource subscriptions in tau-mcp, and the mcp trigger.

Incoming webhooks need a URL the internet can reach, and come later. So
does a headless daemon, with 0013's `tau-host`; nothing here changes
for it.

## Alternatives considered

- **Luau handlers that decide, with the model only on escalation.**
  The previous draft of this decision. Cheaper per event, but the
  agent's behaviour would be code it wrote once; the person wants the
  model to do the work and to change itself as it learns.
- **A side run per event, reporting back.** Runs in parallel and keeps
  the conversation small, but splits the agent's memory across runs.
  One conversation sees everything; sub-agents are there when work
  should go in parallel.
- **An Allow on self-edits that reach further**, as plugins have. Safe,
  but an agent that waits on a click to start watching something new is
  not running on its own. The spend cap and the sandbox are the limits.
- **Reaching the person's repositories.** An agent could fix what it
  finds where it finds it, but it would then edit the person's code
  unattended. It works in its own repository, and tells or asks the
  person.
- **`bash` as any chat has it.** "Its own repository only" would be a
  convention a command can break. `bwrap` makes it a wall.
- **Incoming webhooks now.** They need a public URL, a tunnel or a
  relay. Polls and streams cover most services until then.
- **A daemon now.** Agents would run with the app closed, but the
  daemon is a larger change (0013), and catching up on start covers
  most of what it would.

## Consequences

- tau owns one more repository per agent, each with its conversation,
  workspaces and memory. None pushes anywhere unless the person adds a
  remote.
- An agent costs a turn per coalesced batch of events. A small model,
  the window and the cap keep that bounded; a busy source still costs
  up to the cap each day.
- An agent can change what it watches, its tools, its servers and its
  instructions without the person. What it did is in its jj log and its
  conversation, and the cap and the sandbox are what hold.
- Secrets are a new store, and codemode a new surface (`http`). Both
  are open to agents' runs only.
- `bash` for agents needs `bwrap`. tau's Nix package gains it as a
  runtime dependency; other installs need it on `PATH`.
- A closed tau misses stream frames that the server does not replay
  and MCP notifications; catching up reads the current state instead.
- The Luau interface now has two entries, `tau.plugin` and `tau.agent`,
  sharing `tau`, `tau.test` and codemode. Changes to those are checked
  against both.
