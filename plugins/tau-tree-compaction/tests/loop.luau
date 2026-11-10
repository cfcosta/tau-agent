-- tau-tree-compaction in a real run (ADR 0053): each run steers a second
-- user message in before the first turn ends, so the transcript has a
-- clean cut point: keeping 1 token, the newest user message is kept and
-- everything before it is folded. A turn reporting 5,000 tokens against
-- a 1,000-token window makes compaction due.
local tau = require("tau")
local t = tau.test

-- On, a window of 1,000 tokens, 100 of them reserved, and 1 kept.
local ON = { on = true, window = 1000, reserve_tokens = 100, keep_recent_tokens = 1 }

-- What `read` returns: long enough that its line needs the model.
local function contents(path)
	return "contents of " .. path .. ": " .. string.rep("fn main() {} ", 60)
end

local TOOLS = { read = contents("{path}") }

-- A turn that reads `path` and reports 5,000 tokens.
local function read(path)
	return { { tool = "read", args = { path = path } }, { usage = { input = 5000, output = 10 } } }
end

-- A compactor's answer: a line the test can tell from the others.
local LINE = { text = "echo: read gave 42 bytes" }

local function done(text)
	return { { text = text or "done" }, { usage = { input = 10, output = 10 } } }
end

-- A run of `model`, steering "and then this" in, with `extra` over it.
local function spec(model, extra)
	local out = { prompt = "go", steer = "and then this", settings = ON, tools = TOOLS, model = model }
	for key, value in pairs(extra or {}) do
		out[key] = value
	end
	return out
end

-- A message's text, as tau-ai's `text_of` joins its blocks.
local function text(message)
	if type(message.content) == "string" then
		return message.content
	end
	local parts = {}
	for _, block in message.content do
		if block.type == "text" then
			table.insert(parts, block.text)
		end
	end
	return table.concat(parts, "\n")
end

-- The labelled lines of a view message.
local function lines_of(view)
	local _, at = string.find(view, "<chat>\n", 1, true)
	assert(at ~= nil, "a view: " .. view)
	local body = string.sub(view, at + 1)
	if string.sub(body, -8) == "\n</chat>" then
		body = string.sub(body, 1, -9)
	end
	local lines = {}
	for line in string.gmatch(body, "[^\n]+") do
		table.insert(lines, line)
	end
	return lines
end

local function plugins_of(rewrites)
	local out = {}
	for _, rewrite in rewrites do
		table.insert(out, rewrite.plugin)
	end
	return out
end

local function has(list, name)
	return table.find(list, name) ~= nil
end

local function starts(text, prefix)
	return string.sub(text, 1, #prefix) == prefix
end

local function ends(text, suffix)
	return suffix == "" or string.sub(text, -#suffix) == suffix
end

-- Past the threshold, the messages before the kept one fold into the
-- history: a short one is its own line, word for word, a long one is
-- compressed by a request of its own, and the next turn opens with the
-- view and then the kept message. The run declares `zoom`.
t.loop(
	"older messages fold into the view",
	spec({ read("src/lib.rs"), LINE, done() }, { name = "gpt-5.4-mini", reasoning = "high", clock = 1234 }),
	function(run)
		t.equal(run.outcome.text, "done")
		local rewritten = false
		for _, event in run.events do
			if event.ContextRewritten ~= nil and event.ContextRewritten.plugin == "tau-luau-plugins" then
				rewritten = true
			end
		end
		t.equal(rewritten, true, "a ContextRewritten event of tau-luau-plugins")
		t.equal(plugins_of(run.rewrites), { "tau-tree-compaction" })
		local requests = run.requests
		t.equal(#requests, 3)
		t.equal(has(requests[1].tools, "zoom"), true)
		-- The one request for a line: the result, whole.
		local compactor = requests[2]
		t.equal(starts(compactor.instructions, "You keep the memory of tau"), true, compactor.instructions)
		-- The run's clock stamps what the compactor is sent, and the view.
		t.equal(compactor.messages[1].timestamp, 1234)
		t.equal(requests[3].messages[1].timestamp, 1234)
		-- The run's model and effort build its lines.
		t.equal(compactor.model, "gpt-5.4-mini")
		t.equal(compactor.reasoning, "high")
		t.equal(#compactor.tools, 0)
		t.equal(ends(text(compactor.messages[1]), "echo: [read] " .. contents("src/lib.rs")), true)
		-- The next turn: the view, then the kept message.
		local next = requests[3].messages
		local lines = lines_of(text(next[1]))
		t.equal(lines[1], "0+1|user: go")
		t.equal(lines[2], '1+1|tool: read {"path":"src/lib.rs"}')
		t.equal(starts(lines[3], "2+1|echo: read gave "), true, lines[3])
		t.equal(#lines, 3)
		t.equal(text(next[2]), "and then this")
		t.equal(#next, 2)
		-- What the plugin list reads.
		t.equal(run.state.compacted.messages, 3)
		t.equal(run.state.compacted.asked, 1)
	end
)

-- A second compaction folds only what came since: the history keeps its
-- first messages' lines and ids, the view grows, and the new lines'
-- requests read the old lines as context.
t.loop(
	"a second compaction folds only what came since",
	spec({ read("a.rs"), LINE, read("b.rs"), LINE, done() }, { steer = { "and then this", "and also this" } }),
	function(run)
		t.equal(run.outcome.text, "done")
		local requests = run.requests
		local context = text(requests[4].messages[1])
		t.equal(starts(context, "<chat>\nuser: go\ntool: read"), true, context)
		local view = text(requests[5].messages[1])
		local labels = {}
		for _, line in lines_of(view) do
			local bar = string.find(line, "|", 1, true)
			if bar ~= nil then
				table.insert(labels, string.sub(line, 1, bar - 1))
			end
		end
		-- go, read a, its result; and then this, read b, its result.
		t.equal(labels, { "0+1", "1+1", "2+1", "3+1", "4+1", "5+1" })
		t.equal(string.find(view, "3+1|user: and then this", 1, true) ~= nil, true, view)
		-- The second compaction says it folded three messages, not counting
		-- the view it opened with.
		t.equal(run.state.compactions, 2)
		t.equal(run.state.compacted.messages, 3)
	end
)

-- A fork of a compacted run starts from the view and can still zoom into
-- what was folded, down to a message whole: its history comes back from
-- the state it inherits.
t.loop(
	"a fork of a compacted run can zoom into the history",
	spec({
		read("src/lib.rs"),
		LINE,
		done(),
		{ { tool = "zoom", args = { id = 2, n = 1 } }, { usage = { input = 10, output = 10 } } },
		{ { tool = "zoom", args = { id = 0, n = 2 } }, { usage = { input = 10, output = 10 } } },
		{ { tool = "zoom", args = { id = 1, n = 2 } }, { usage = { input = 10, output = 10 } } },
		done("forked"),
	}, { ["then"] = { { prompt = "and now" } } }),
	function(run)
		t.equal(run.forks[1].outcome.text, "forked")
		local requests = run.requests
		t.equal(#requests, 7)
		-- The fork opens with the base's view.
		t.equal(requests[4].messages[1], requests[3].messages[1])
		-- A leaf opens to its message whole, a pair to its two lines, and a
		-- line that is not one says so.
		local results = {}
		for _, message in requests[#requests].messages do
			if message.role == "toolResult" then
				table.insert(results, text(message))
			end
		end
		t.equal(results, {
			"2+1|echo: [read] " .. contents("src/lib.rs"),
			'0+1|user: go\n1+1|tool: read {"path":"src/lib.rs"}',
			"No line 1+2.",
		})
	end
)

-- Below the threshold nothing is folded, and `zoom` says there is
-- nothing to open.
t.loop(
	"below the threshold nothing folds",
	spec({
		{ { tool = "zoom", args = { id = 0, n = 1 } }, { usage = { input = 10, output = 10 } } },
		done(),
	}),
	function(run)
		t.equal(run.outcome.text, "done")
		t.equal(#run.rewrites, 0)
		local last = run.requests[#run.requests].messages
		local result
		for _, message in last do
			if message.role == "toolResult" then
				result = message
				break
			end
		end
		assert(result ~= nil, "the zoom's result")
		t.equal(result.isError, true)
		t.equal(text(result), "Nothing has been compacted yet: there is no line to open.")
	end
)

-- Off, the run is not offered `zoom`, and nothing folds.
t.loop(
	"off there is no zoom",
	spec({ read("src/lib.rs"), done() }, {
		settings = { on = false, window = 1000, reserve_tokens = 100, keep_recent_tokens = 1 },
	}),
	function(run)
		t.equal(run.outcome.text, "done")
		t.equal(has(run.requests[1].tools, "zoom"), false)
		t.equal(#run.rewrites, 0)
		t.equal(#run.requests, 2)
	end
)

-- The stored rewrite holds the view and how many entries it covers, and
-- its message is the view message the next turn opens with.
t.loop("the rewrite keeps the view", spec({ read("src/lib.rs"), LINE, done() }), function(run)
	local first = run.transcript[1]
	t.equal(first.kind, "context")
	t.equal(first.plugin, "tau-luau-plugins")
	t.equal(first.body.plugin, "tau-tree-compaction")
	t.equal(#run.rewrites, 1)
	local rewrite = run.rewrites[1]
	t.equal(rewrite.plugin, "tau-tree-compaction")
	local details = rewrite.details
	t.equal(details.entries, 3)
	t.equal(#details.view, 3)
	t.equal(details.tokens_before >= 5010, true, tostring(details.tokens_before))
	local next = run.requests[3].messages
	t.equal(#run.state.history.entries, 3)
	t.equal(run.state.history.view, details.view)
	local view = text(next[1])
	t.equal(ends(view, "<chat>\n" .. table.concat(lines_of(view), "\n") .. "\n</chat>"), true, view)
end)

-- After a failed compaction, tree compaction waits two turns, then tries
-- again: a failing compactor is not paid for every turn.
t.loop(
	"a failed compaction is tried again after a wait",
	spec({
		read("a.rs"),
		{ error = { code = "invalid_prompt", message = "refused" } },
		-- Turn 2 is inside the wait: nothing is asked.
		read("b.rs"),
		read("c.rs"),
		-- Turn 3: the results of a.rs and b.rs need a line each.
		LINE,
		LINE,
		done(),
	}),
	function(run)
		t.equal(run.outcome.text, "done")
		t.equal(plugins_of(run.rewrites), { "tau-tree-compaction" })
		t.equal(#run.requests, 7)
	end
)

-- When a line cannot be built, the tree writes nothing and
-- tau-compaction, after it, summarizes instead.
t.loop(
	"a failed tree leaves the summary to tau-compaction",
	spec({
		read("src/lib.rs"),
		{ error = { code = "invalid_prompt", message = "refused" } },
		{ text = "## Goal\nship it" },
		done(),
	}, {
		plugins = {
			{ name = "tau-compaction", settings = { window = 1000, reserve_tokens = 100, keep_recent_tokens = 1 } },
		},
	}),
	function(run)
		t.equal(run.outcome.text, "done")
		t.equal(plugins_of(run.rewrites), { "tau-compaction" })
		local logged = false
		for _, line in run.logs do
			if starts(line, "the compactor failed on") then
				logged = true
			end
		end
		t.equal(logged, true, "the tree's failure is in its log: " .. json.encode(run.logs))
		t.equal(#run.requests, 4)
	end
)

-- No more than `jobs` lines are asked for at once, and every slot is
-- used when there is work for it: one turn reads `count` files, each
-- result needing a line, and each line takes 50 ms to come.
for _, case in { { jobs = 1, count = 3 }, { jobs = 3, count = 7 }, { jobs = 6, count = 20 } } do
	local jobs, count = case.jobs, case.count
	local reads = {}
	for at = 1, count do
		table.insert(reads, { tool = "read", args = { path = "f" .. at .. ".rs" } })
	end
	table.insert(reads, { usage = { input = 5000, output = 10 } })
	local model = { reads }
	for _ = 1, count do
		table.insert(model, { LINE, { delay = 50 } })
	end
	table.insert(model, done())
	local settings = { on = true, window = 1000, reserve_tokens = 100, keep_recent_tokens = 1, jobs = jobs, view_bytes = 1000000 }
	t.loop(
		"no more than " .. jobs .. " requests run at once, " .. count .. " lines asked",
		spec(model, { steer = "next", settings = settings }),
		function(run)
			t.equal(run.outcome.text, "done")
			t.equal(plugins_of(run.rewrites), { "tau-tree-compaction" })
			t.equal(run.state.compacted.asked, count)
			t.equal(#run.requests, count + 2)
			t.equal(run.most_at_once, jobs, run.most_at_once .. " at once, " .. jobs .. " allowed")
		end
	)
end
