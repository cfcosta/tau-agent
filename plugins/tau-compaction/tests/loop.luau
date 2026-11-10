-- tau-compaction in a real agent loop (ADR 0053), its Luau run as tau
-- runs it. Most runs steer a second user message in before the first
-- turn ends, so the transcript has a clean cut point: keeping 1 token,
-- the newest user message is kept and everything before it is
-- summarized.
local tau = require("tau")
local t = tau.test
local compaction = require("compaction")

-- A window of 1,000 tokens, 100 of them reserved, and 1 kept.
local SETTINGS = { window = 1000, reserve_tokens = 100, keep_recent_tokens = 1 }

-- A window no turn passes: only an overflow compacts.
local WIDE = 1000000000000
local WIDE_SETTINGS = { window = WIDE, reserve_tokens = 100, keep_recent_tokens = 1 }

local TOOLS = { read = "contents of {path}" }

local function contains(text, part)
	return string.find(text, part, 1, true) ~= nil
end

local function starts_with(text, part)
	return string.sub(text, 1, #part) == part
end

local function ends_with(text, part)
	return string.sub(text, -#part) == part
end

-- A turn reading `path`, at 5,000 tokens in when `big`.
local function read(path, big)
	if big then
		return { { tool = "read", args = { path = path } }, { usage = { input = 5000, output = 10 } } }
	end
	return { tool = "read", args = { path = path } }
end

-- A user message's text.
local function text(message)
	t.equal(message.role, "user", "a user message")
	t.equal(type(message.content), "string", "a text message")
	return message.content
end

-- The run's `ContextRewritten` events.
local function rewritten(run)
	local out = {}
	for _, event in ipairs(run.events) do
		if event.ContextRewritten then
			table.insert(out, event.ContextRewritten)
		end
	end
	return out
end

-- The kinds of `run`'s stored transcript, in order.
local function kinds(run)
	local out = {}
	for _, entry in ipairs(run.transcript) do
		table.insert(out, entry.kind)
	end
	return out
end

-- The message of a run that stopped with an error.
local function failed(run)
	local stop = run.outcome.stop
	t.equal(type(stop), "table", "an error stop")
	t.equal(type(stop.Error), "string", "an error stop")
	return stop.Error
end

-- Past the threshold, the loop summarizes everything before the kept
-- messages in a request of its own, with the system prompt, no tools and
-- the serialized conversation, then goes on from the summary and the kept
-- message; the store holds the compaction record, pointing at the kept
-- message, and the summary; a `ContextRewritten` event reports the size
-- before; the summary's usage counts toward the run.
t.loop("threshold compaction summarizes older messages", {
	settings = SETTINGS,
	name = "gpt-5.4-mini",
	reasoning = "high",
	prompt = "go",
	steer = "and then this",
	tools = TOOLS,
	model = {
		{ { tool = "read", args = { path = "src/lib.rs" } }, { usage = { input = 5000, output = 10 } }, { cost = 0.5 } },
		{ { text = "## Goal\nship it" }, { cost = 0.25 } },
		{ text = "done" },
	},
}, function(run)
	t.equal(run.outcome.stop, "Stop")
	t.equal(run.outcome.text, "done")
	t.equal(run.outcome.usage.cost.total, 0.75)
	local events = rewritten(run)
	t.equal(#events, 1, "rewrites")
	t.equal(events[1].plugin, "tau-luau-plugins")
	t.equal(events[1].tokens_before >= 5010, true, tostring(events[1].tokens_before))

	t.equal(#run.requests, 3, "requests")
	local summary = run.requests[2]
	t.equal(summary.instructions, compaction.SYSTEM_PROMPT)
	t.equal(summary.tools, {})
	t.equal(summary.model, "gpt-5.4-mini")
	t.equal(summary.settings.reasoning, "high")
	t.equal(#summary.messages, 1, "summary messages")
	local asked = text(summary.messages[1])
	t.equal(starts_with(asked, "<conversation>\n[User]: go"), true, asked)
	t.equal(contains(asked, 'read(path="src/lib.rs")'), true, asked)
	t.equal(contains(asked, "and then this"), false, asked)

	local next = run.requests[3].messages
	t.equal(#next, 2, "next messages")
	local opening = text(next[1])
	t.equal(starts_with(opening, compaction.SUMMARY_PREFIX), true, opening)
	t.equal(contains(opening, "## Goal\nship it"), true, opening)
	t.equal(contains(opening, "<read-files>\nsrc/lib.rs\n</read-files>"), true, opening)
	t.equal(text(next[2]), "and then this")

	t.equal(#run.rewrites, 1, "stored rewrites")
	t.equal(run.rewrites[1].plugin, "tau-compaction")
	local record = run.rewrites[1].details
	t.equal(opening, compaction.SUMMARY_PREFIX .. record.summary .. "\n</summary>")
	t.equal(record.read_files, { "src/lib.rs" })
	-- The context entry, the summary, the kept message, the answer.
	t.equal(kinds(run), { "context", "message", "message", "message" })
	t.equal(run.transcript[1].plugin, "tau-luau-plugins")
	t.equal(run.record.cost_usd, 0.75)
end)

-- A second compaction updates the first summary: it uses the "update"
-- prompt with the previous summary, never summarizes the previous summary
-- message as conversation, and carries the file lists forward.
t.loop("a second compaction updates the first summary", {
	settings = SETTINGS,
	prompt = "go",
	steer = { "then b", "then finish" },
	tools = TOOLS,
	model = {
		read("a.rs", true),
		{ text = "first summary" },
		read("b.rs", true),
		{ text = "second summary" },
		{ text = "done" },
	},
}, function(run)
	t.equal(run.outcome.text, "done")
	local update = text(run.requests[4].messages[1])
	t.equal(ends_with(update, compaction.UPDATE_SUMMARIZATION_PROMPT), true, update)
	t.equal(contains(update, "<previous-summary>\nfirst summary"), true, update)
	t.equal(contains(update, compaction.SUMMARY_PREFIX), false, update)
	local last = text(run.requests[5].messages[1])
	t.equal(contains(last, "second summary"), true, last)
	t.equal(contains(last, "<read-files>\na.rs\nb.rs\n</read-files>"), true, last)
	-- Loading the run drops everything before the latest record.
	local contexts = 0
	for _, kind in ipairs(kinds(run)) do
		if kind == "context" then
			contexts += 1
		end
	end
	t.equal(contexts, 1, "stored rewrites")
end)

-- When the cut falls inside the only turn, its start is summarized with
-- the turn-prefix prompt and merged under the history, "No prior
-- history.". Keeping 10 tokens keeps the last tool call and its result:
-- the cut lands on the second assistant message.
t.loop("a split turn gets a prefix summary", {
	settings = { window = 1000, reserve_tokens = 100, keep_recent_tokens = 10 },
	prompt = "go",
	tools = TOOLS,
	model = {
		read("a.rs", true),
		read("b.rs", true),
		{ text = "prefix summary" },
		{ text = "done" },
	},
}, function(run)
	t.equal(run.outcome.text, "done")
	local asked = text(run.requests[3].messages[1])
	t.equal(ends_with(asked, compaction.TURN_PREFIX_SUMMARIZATION_PROMPT), true, asked)
	local opening = text(run.requests[4].messages[1])
	t.equal(
		contains(opening, "No prior history.\n\n---\n\n**Turn Context (split turn):**\n\nprefix summary"),
		true,
		opening
	)
end)

-- A context overflow compacts once and retries the turn once; the
-- threshold never fires, the window is huge. The failed response is not
-- stored.
t.loop("an overflow compacts and retries once", {
	settings = WIDE_SETTINGS,
	prompt = "go",
	steer = "then this",
	tools = TOOLS,
	model = {
		read("a.rs"),
		{ error = { code = "context_length_exceeded", message = "Your input exceeds the context window of this model" } },
		{ text = "summary" },
		{ text = "done" },
	},
}, function(run)
	t.equal(run.outcome.stop, "Stop")
	t.equal(run.outcome.text, "done")
	t.equal(#run.requests, 4, "requests")
	t.equal(#run.requests[4].messages, 2, "messages retried")
	local errors = 0
	for _, entry in ipairs(run.transcript) do
		if entry.kind == "message" and entry.message.stopReason == "error" then
			errors += 1
		end
	end
	t.equal(errors, 0, "stored failures")
end)

-- A second overflow after compacting fails the run with it.
t.loop("a second overflow fails the run", {
	settings = WIDE_SETTINGS,
	prompt = "go",
	steer = "then this",
	tools = TOOLS,
	model = {
		read("a.rs"),
		{ error = { code = "context_length_exceeded", message = "too long" } },
		{ text = "summary" },
		{ error = { code = "context_length_exceeded", message = "too long" } },
	},
}, function(run)
	local message = failed(run)
	t.equal(contains(message, "context_length_exceeded"), true, message)
	-- Every scripted turn was drawn.
	t.equal(#run.requests, 4, "requests")
end)

-- A rejected summary on overflow writes nothing and fails the run with
-- both errors: here the summary hit the token cap.
t.loop("a rejected summary on overflow fails the run", {
	settings = WIDE_SETTINGS,
	prompt = "go",
	steer = "then this",
	tools = TOOLS,
	model = {
		read("a.rs"),
		{ error = { code = "context_length_exceeded", message = "too long" } },
		{ { text = "cut off" }, { stop = "length" } },
	},
}, function(run)
	local message = failed(run)
	t.equal(contains(message, "context_length_exceeded"), true, message)
	t.equal(contains(message, "tau-compaction: Summarization failed: generation hit the token cap"), true, message)
	t.equal(#rewritten(run), 0, "rewrite events")
	for at, kind in ipairs(kinds(run)) do
		t.equal(kind, "message", "entry " .. at)
	end
end)

-- A rejected summary past the threshold writes nothing: the run goes on
-- uncompacted, the turn after it sees the whole transcript, and the next
-- turn does not try again. Compaction waits two turns, then tries again:
-- a run with no turn cap is not left uncompacted for good.
t.loop("a rejected summary is tried again after a wait", {
	settings = SETTINGS,
	prompt = "go",
	steer = "then this",
	tools = TOOLS,
	model = {
		read("a.rs", true),
		-- Rejected: a summary is text.
		read("x"),
		read("b.rs", true),
		read("c.rs", true),
		-- The history's summary, then the split turn's prefix.
		{ text = "## Goal\nship it" },
		{ text = "## Original Request\nread b.rs" },
		{ text = "done" },
	},
}, function(run)
	t.equal(run.outcome.text, "done")
	t.equal(#rewritten(run), 1, "rewrite events")
	-- The turn after the rejected summary saw the whole transcript.
	t.equal(text(run.requests[3].messages[1]), "go")
	-- Every scripted turn was drawn.
	t.equal(#run.requests, 7, "requests")
end)

-- A fork of a compacted run starts from the summary: the store drops
-- everything before the record, and the fork's first request is the
-- summary, the kept messages, and the fork's input.
t.loop("a fork of a compacted run starts from the summary", {
	settings = SETTINGS,
	prompt = "go",
	steer = "then this",
	tools = TOOLS,
	model = {
		read("a.rs", true),
		{ text = "summary" },
		{ text = "done" },
		{ text = "forked" },
	},
	["then"] = { { prompt = "and now" } },
}, function(run)
	t.equal(run.forks[1].outcome.text, "forked")
	local seen = run.requests[4].messages
	local before = run.requests[3].messages
	t.equal({ seen[1], seen[2] }, { before[1], before[2] })
	t.equal(text(seen[4]), "and now")
	t.equal(#seen, 4, "fork messages")
end)

-- An overflow is recognized by its wording too, when the failure has no
-- code or class that says so.
t.loop("an overflow is recognized by its wording", {
	settings = WIDE_SETTINGS,
	prompt = "go",
	steer = "then this",
	tools = TOOLS,
	model = {
		read("a.rs"),
		{ failure = { message = "Your input exceeds the context window of this model", class = "fatal" } },
		{ text = "summary" },
		{ text = "done" },
	},
}, function(run)
	t.equal(run.outcome.text, "done")
	t.equal(#rewritten(run) > 0, true, "a rewrite event")
end)

-- A context overflow reported by its class alone, in words that do not
-- say so, compacts, and the summary request goes through the retry
-- policy too.
t.loop("an overflow code compacts and the summary is retried", {
	settings = { window = WIDE, keep_recent_tokens = 1 },
	prompt = "go",
	steer = "more",
	model = {
		{ text = "first" },
		{ failure = { message = "no room", class = "context_overflow" } },
		{ error = { code = "server_error", message = "try again" } },
		{ text = "summary" },
		{ text = "done" },
	},
}, function(run)
	t.equal(run.outcome.text, "done")
	t.equal(#rewritten(run) > 0, true, "a rewrite event")
	-- Every scripted turn was drawn.
	t.equal(#run.requests, 5, "requests")
end)

-- A summary request that keeps failing retryably is tried as many times
-- as the policy allows, and then compaction fails.
t.loop("a failing summary is tried as the policy allows", {
	settings = { window = WIDE, keep_recent_tokens = 1 },
	retry = { max_attempts = 2 },
	prompt = "go",
	steer = "more",
	model = {
		{ text = "first" },
		{ error = { code = "context_length_exceeded", message = "no room" } },
		{ error = { code = "server_error", message = "try again" } },
		{ error = { code = "server_error", message = "try again" } },
		{ text = "never" },
	},
}, function(run)
	local message = failed(run)
	t.equal(contains(message, "tau-compaction: Summarization failed: server_error"), true, message)
	-- One turn left: "never".
	t.equal(#run.requests, 4, "requests")
end)

-- An idle run is summarized in its own conversation: the request goes
-- with the settings of the run's last request, its input the messages it
-- summarizes as they are, then the in-context prompt. The rewrite is
-- stored as an idle one, with no message added and the run left
-- finished; it goes on from the summary, and a second one updates the
-- summary the conversation opens with. No window is known: nothing
-- compacts at a turn's end.
t.loop("an idle run is summarized in its own conversation", {
	settings = { keep_recent_tokens = 1 },
	prompt = "go",
	steer = "and then this",
	tools = TOOLS,
	model = {
		{ text = "first" },
		{ text = "done" },
		{ text = "## Goal\nidle summary" },
		{ text = "again" },
		{ text = "## Goal\nupdated summary" },
	},
	["then"] = { { idle = true }, { resume = true, prompt = "next" }, { idle = true } },
}, function(run)
	local idle = run.forks[1]
	t.equal(idle.compacted.plugin, "tau-luau-plugins")
	local summary = run.requests[3]
	t.equal(summary.settings, run.outcome.request)
	-- All but the reply it keeps: what the last request sent.
	t.equal({ summary.messages[1], summary.messages[2], summary.messages[3] }, run.requests[2].messages)
	local prompt = text(summary.messages[4])
	t.equal(starts_with(prompt, compaction.IN_CONTEXT_PREAMBLE), true, prompt)
	t.equal(ends_with(prompt, compaction.SUMMARIZATION_PROMPT), true, prompt)
	t.equal(#summary.messages, 4, "summary messages")

	t.equal({ idle.record.status, idle.record.turns }, { idle.before.status, idle.before.turns })
	t.equal(idle.transcript[1].kind, "context")
	t.equal(idle.transcript[1].trigger, "idle")
	t.equal(#idle.transcript, 3, "entries")
	t.equal(idle.transcript[2].kind, "message")
	t.equal(contains(text(idle.transcript[2].message), "idle summary"), true)
	local reply = idle.transcript[3]
	t.equal(reply.kind, "message")
	t.equal(reply.message.role, "assistant")
	t.equal(reply.message.content, { { type = "text", text = "done" } })

	-- It goes on from the summary, and the next idle compaction updates it.
	t.equal(contains(text(run.requests[4].messages[1]), "idle summary"), true)
	t.equal(type(run.forks[3].compacted), "table", "compacted again")
	local update = run.requests[5].messages
	t.equal(contains(text(update[1]), "idle summary"), true)
	local last = text(update[#update])
	t.equal(contains(last, "existing summary in <summary> tags at the start of the conversation"), true, last)
end)

-- With idle compaction off, an idle run is left as it is: no request,
-- nothing stored.
t.loop("idle compaction off leaves the run", {
	settings = { keep_recent_tokens = 1, idle = false },
	prompt = "go",
	steer = "and then this",
	model = {
		{ text = "first" },
		{ text = "done" },
	},
	["then"] = { { idle = true } },
}, function(run)
	local idle = run.forks[1]
	t.equal(idle.compacted, json.null)
	t.equal(#run.requests, 2, "requests")
	for at, kind in ipairs(kinds(idle)) do
		t.equal(kind, "message", "entry " .. at)
	end
end)
