-- tau-memory in a real agent loop (ADR 0053): a chat, compaction by
-- tau-compaction, edits and commits, and the pass after a run.
local tau = require("tau")
local t = tau.test
local note = require("note")
local render = require("render")

-- No pass after the run, unless a test asks for one.
local NO_PASS = { after_each_run = false }

-- tau-compaction, summarizing whatever a turn leaves past 1,000 tokens.
local COMPACTION = { name = "tau-compaction", settings = { window = 1000, reserve_tokens = 100, keep_recent_tokens = 1 } }

local function contains(s, part)
	return string.find(s, part, 1, true) ~= nil
end

-- How often `part` is in `s`.
local function times(s, part)
	local n, at = 0, 1
	while true do
		local found = string.find(s, part, at, true)
		if found == nil then
			return n
		end
		n += 1
		at = found + #part
	end
end

-- A request's messages, as JSON.
local function asked(request)
	return json.encode(request.messages)
end

-- The first message of a request: its text.
local function first(request)
	local content = request.messages[1].content
	if type(content) == "string" then
		return content
	end
	return json.encode(content)
end

local function file(kind, id, title, body, extra)
	local n = {
		id = id,
		title = title,
		description = "about " .. title,
		type = kind,
		tags = {},
		created = 1,
		updated = 1,
		valid_from = 1,
		source = note.source("user"),
		links = {},
		body = body,
	}
	for key, value in pairs(extra or {}) do
		n[key] = value
	end
	return note.render(n)
end

local function stored(run, kind, id)
	local found = run.repository["tau-memory/" .. kind .. "/" .. id .. ".md"]
	return found and note.parse(found)
end

local RETRY = {
	["tau-memory/index/index.md"] = file("index", "index", "Index", "- `retry-after`: HTTP dates too"),
	["tau-memory/gotcha/retry-after.md"] = file("gotcha", "retry-after", "Retry after", "retry-after can be an HTTP date"),
}

-- Each message of a chat is a run, and each starts with memory: the
-- first with the index and the notes for its task, fenced, and the
-- memory tools; a note the chat already has stays out, and so does the
-- index until it changes.
t.loop("a chat gets each note and the index once", {
	settings = NO_PASS,
	repository = RETRY,
	prompt = "fix the retry after parsing",
	model = {
		{ text = "done" },
		{ tool = "memory_write", args = { type = "index", title = "Index", description = "map of notes", body = "- `retry-after`: seconds or a date" } },
		{ text = "done too" },
		{ text = "done again" },
	},
	["then"] = {
		{ resume = true, prompt = "and the retry after header" },
		{ resume = true, prompt = "now the retry after tests" },
	},
}, function(run)
	t.equal(#run.requests, 4, "requests")
	local opening = first(run.requests[1])
	t.equal(contains(opening, "<memory"), true, opening)
	t.equal(contains(opening, "HTTP dates too"), true, "the index note")
	t.equal(contains(opening, "`retry-after` [gotcha]"), true, "the hit")
	local tools = {}
	for _, tool in ipairs(run.requests[1].tools) do
		table.insert(tools, type(tool) == "table" and tool.name or tool)
	end
	for _, name in ipairs({ "memory_write", "memory_search", "memory_read", "memory_link" }) do
		t.equal(table.find(tools, name) ~= nil, true, name)
	end
	-- The second message: neither the note nor the index again.
	local second = asked(run.requests[3])
	t.equal(times(second, "`retry-after` [gotcha]"), 1, "the note, once")
	t.equal(times(second, "HTTP dates too"), 1, "the index, once")
	-- The index changed: the third message has it again, as it is now.
	local third = asked(run.requests[4])
	t.equal(times(third, '<index scope=\\"repository\\">'), 2, "the index, twice: as it was and as it is")
	t.equal(times(third, "`retry-after` [gotcha]"), 1, "the note, still once")
end)

-- When compaction drops the conversation, one request with only the
-- memory tools sees what was dropped, and its writes are carried out, as
-- the agent's.
t.loop("compaction gives memory one request over what it drops", {
	settings = NO_PASS,
	plugins = { COMPACTION },
	prompt = "why do lanes stall",
	steer = "keep going",
	model = {
		{ { tool = "memory_search", args = { query = "lanes" } }, { usage = { input = 5000, output = 10 } } },
		{ text = "## Goal\nfind the stall" },
		{
			tool = "memory_write",
			args = {
				type = "gotcha",
				title = "Lanes lose their continuation on drain",
				description = "a draining socket drops the delta chain",
				body = "Seen in the run: resend in full after a drain.",
			},
		},
		{ text = "done" },
	},
}, function(run)
	t.equal(run.outcome.text, "done")
	t.equal(#run.requests, 4, "the turn, the summary, the flush, the next turn")
	local flush = run.requests[3]
	local names = {}
	for _, tool in ipairs(flush.tools) do
		table.insert(names, type(tool) == "table" and tool.name or tool)
	end
	t.equal(names, { "memory_write", "memory_search", "memory_read", "memory_link" })
	t.equal(flush.instructions, render.FLUSH_PROMPT)
	local shown = first(flush)
	t.equal(contains(shown, "why do lanes stall") and contains(shown, "[call memory_search]"), true, shown)
	local n = stored(run, "gotcha", "lanes-lose-their-continuation-on-drain")
	t.equal(n.source.by, "agent")
	local saved = false
	for _, entry in ipairs(run.state.log or {}) do
		saved = saved or entry.kind == "saved"
	end
	t.equal(saved, true, "what it saved is in the state")
end)

-- Compaction that drops the conversation drops the notes it was given:
-- the chat's next message gets them again.
t.loop("notes dropped by compaction are given again", {
	settings = NO_PASS,
	plugins = { COMPACTION },
	repository = RETRY,
	prompt = "fix the retry after parsing",
	steer = "keep going",
	model = {
		{ { tool = "memory_search", args = { query = "retry" } }, { usage = { input = 5000, output = 10 } } },
		{ text = "## Goal\nfix it" },
		-- The flush, with nothing to write.
		{ text = "nothing new" },
		{ text = "done" },
		{ text = "done too" },
	},
	["then"] = { { resume = true, prompt = "and the retry after header" } },
}, function(run)
	t.equal(#run.requests, 5, "requests")
	t.equal(times(asked(run.requests[5]), "`retry-after` [gotcha]"), 1, "given again, once")
end)

-- The flush runs on the model and effort memory's settings name (luna,
-- low, by default), and on the run's own with the chat's model chosen.
for _, case in ipairs({
	{ name = "the settings' model", settings = NO_PASS, family = "luna", effort = "low" },
	{ name = "the chat's model", settings = { after_each_run = false, follow_chat = true }, model = "gpt-5.4-mini", effort = "high" },
}) do
	t.loop("the flush writes with " .. case.name, {
		settings = case.settings,
		plugins = { COMPACTION },
		name = "gpt-5.4-mini",
		reasoning = "high",
		prompt = "why do lanes stall",
		steer = "keep going",
		model = {
			{ { tool = "memory_search", args = { query = "lanes" } }, { usage = { input = 5000, output = 10 } } },
			{ text = "## Goal\nfind the stall" },
			{ text = "nothing to keep" },
			{ text = "done" },
		},
	}, function(run)
		local flush = run.requests[3]
		t.equal(flush.instructions, render.FLUSH_PROMPT)
		if case.family then
			t.equal(contains(flush.model, case.family), true, flush.model)
		else
			t.equal(flush.model, case.model)
		end
		t.equal(flush.settings.reasoning, case.effort)
		-- The run itself keeps its own.
		t.equal(run.requests[4].model, run.requests[1].model)
		t.equal(run.requests[4].instructions == render.FLUSH_PROMPT, false)
	end)
end

-- A rewrite that keeps the conversation, as pruning does, asks memory
-- for nothing: under its window, tau-compaction does not rewrite, and
-- the model is asked only the run's turns.
t.loop("a run that does not compact asks memory nothing", {
	settings = NO_PASS,
	plugins = { COMPACTION },
	prompt = "why do lanes stall",
	model = { { tool = "memory_search", args = { query = "lanes" } }, { text = "done" } },
}, function(run)
	t.equal(#run.requests, 2)
end)

t.loop("editing a file marks the notes about it", {
	settings = NO_PASS,
	repository = {
		["tau-memory/fact/retry-reads-the-header.md"] = file(
			"fact",
			"retry-reads-the-header",
			"Retry reads the header",
			"in retry.rs",
			{ links = { { to = "src/retry.rs", type = "about" } } }
		),
		["tau-memory/fact/unrelated.md"] = file("fact", "unrelated", "Unrelated", "elsewhere"),
	},
	tools = { edit = "edited {path}" },
	prompt = "change retry",
	model = { { tool = "edit", args = { path = "src/retry.rs" } }, { text = "done" } },
}, function(run)
	t.equal(stored(run, "fact", "retry-reads-the-header").stale, "src/retry.rs was edited after this note was written")
	t.equal(stored(run, "fact", "unrelated").stale, nil)
end)

-- A turn's commit marks the notes about what it changed that were
-- written before the turn began; one written in the turn is left alone:
-- the change may have come before it.
t.loop("a turn's commit marks the notes written before it", {
	settings = NO_PASS,
	repository = {
		["tau-memory/fact/retry-reads-the-header.md"] = file(
			"fact",
			"retry-reads-the-header",
			"Retry reads the header",
			"in retry.rs",
			{ links = { { to = "src/retry.rs", type = "about" } } }
		),
	},
	commits = { { turn = 1, change = "kxqvzmnwlroptsuy", paths = { "src/retry.rs" } } },
	prompt = "change retry",
	model = {
		{
			-- Later than the run's start, by a clear margin.
			{ delay = 5 },
			{
				tool = "memory_write",
				args = {
					type = "fact",
					title = "Retry waits a second",
					description = "d",
					body = "b",
					links = { { to = "src/retry.rs", type = "about" } },
				},
			},
		},
		{ text = "done" },
	},
}, function(run)
	t.equal(stored(run, "fact", "retry-reads-the-header").stale, "commit kxqvzmnwlrop changed it")
	t.equal(stored(run, "fact", "retry-waits-a-second").stale, nil, "written in the turn")
end)

-- The pass after a run: a search before a write comes back to the model,
-- which then writes, and a round of only writes ends it; its notes are
-- inferred.
t.loop("consolidation writes after it searched", {
	prompt = "fix the flaky lane test",
	model = {
		{ text = "fixed" },
		{ tool = "memory_search", args = { query = "flaky lane test" } },
		{
			tool = "memory_write",
			args = {
				type = "case",
				title = "Fixed a flaky lane test",
				description = "a race on drain; resend in full",
				body = "Task: flaky lane test. Outcome: fixed.",
			},
		},
	},
}, function(run)
	t.equal(#run.requests, 3, "the run, then the pass's two rounds")
	t.equal(run.requests[2].instructions, render.CONSOLIDATE_PROMPT)
	t.equal(times(asked(run.requests[2]), '"role":"toolResult"'), 0, "the first round")
	t.equal(times(asked(run.requests[3]), '"role":"toolResult"'), 1, "the search came back")
	local count = 0
	for path in pairs(run.repository) do
		if string.sub(path, 1, 11) == "tau-memory/" and not contains(path, "/.history/") then
			count += 1
		end
	end
	t.equal(count, 1, "one note, from the write")
	local n = stored(run, "case", "fixed-a-flaky-lane-test")
	t.equal(n.source.by, "inferred", "the pass's notes are inferred")
	t.equal({ n.title, n.description, n.type, n.body }, { "Fixed a flaky lane test", "a race on drain; resend in full", "case", "Task: flaky lane test. Outcome: fixed." })
end)

-- Each message of a chat is a run: the pass after it reads what that run
-- added, from its message on, not the history it went on from.
t.loop("the pass after a run reads only what the run added", {
	prompt = "why do lanes stall",
	model = {
		{ text = "the lanes stall on drain" },
		-- The pass after the first run.
		{ text = "nothing to keep" },
		{ text = "resend in full after a drain" },
		-- The pass after the second.
		{ text = "nothing to keep" },
	},
	["then"] = { { prompt = "and how do we fix it" } },
}, function(run)
	t.equal(#run.requests, 4)
	local after_first = first(run.requests[2])
	t.equal(contains(after_first, "why do lanes stall"), true, after_first)
	local after_second = first(run.requests[4])
	t.equal(contains(after_second, "and how do we fix it"), true, after_second)
	t.equal(contains(after_second, "resend in full after a drain"), true, after_second)
	t.equal(contains(after_second, "why do lanes stall"), false, after_second)
end)
