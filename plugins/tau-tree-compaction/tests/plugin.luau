-- tau-tree-compaction in a run: off until switched on; past the
-- threshold the messages before the kept one fold into the history and
-- the view takes their place; a second compaction folds only what came
-- since; an idle chat is the summary's; an overflow folds whatever the
-- window; a failed line writes nothing and waits; and `zoom` opens what
-- was folded.
local tau = require("tau")
local t = tau.test
local tree = require("tree")
local build = require("build")

local ON = { on = true, reserve_tokens = 100, keep_recent_tokens = 1, window = 1000, view_bytes = 64000, jobs = 8 }

local function contents(path)
	return "contents of " .. path .. ": " .. string.rep("fn main() {} ", 60)
end

local function user(text)
	return { role = "user", text = text, tokens = 1 }
end

local function read(id, path)
	return {
		role = "assistant",
		text = "",
		tokens = 1,
		calls = { { id = id, name = "read", args = { path = path }, order = { "path" }, json = '{"path":"' .. path .. '"}' } },
	}
end

local function result(id, path)
	return { role = "toolResult", text = contents(path), tokens = 1, call = id, name = "read", ok = true }
end

local function view(messages, spec)
	spec = spec or {}
	return {
		trigger = spec.trigger or "turn_end",
		turn = spec.turn or 1,
		tokens = spec.tokens or 5000,
		window = spec.window,
		messages = messages,
	}
end

-- A run whose compactor answers a line naming the step it was asked,
-- or with `answer`, keeping what it was asked.
local function folding(spec)
	spec = spec or {}
	local asked = {}
	local run = t.run({
		settings = spec.settings or ON,
		state = spec.state,
		tools = {
			infer = function(request)
				table.insert(asked, request)
				if spec.answer then
					return spec.answer(request, #asked)
				end
				local lines = string.split(request.task[2], "\n")
				return { ok = true, value = "echo: read gave " .. #lines[#lines] .. " bytes" }
			end,
		},
	})
	return run, asked
end

local function first_run()
	return { user("go"), read("c1", "src/lib.rs"), result("c1", "src/lib.rs"), user("and then this") }
end

-- The labelled lines of a view message.
local function lines_of(text)
	local body = string.match(text, "<chat>\n(.*)\n</chat>$")
	return string.split(body, "\n")
end

t.case("off by default, it folds nothing and says so", function()
	local run, asked = folding({ settings = t.load().settings.default })
	t.equal(run:rewrite_context(view(first_run())), nil)
	t.equal(#asked, 0)
	t.equal(run:view().status.text, "off")
	t.equal(t.load().settings.default.on, false)
end)

t.case("past the threshold, the older messages fold into the view", function()
	local run, asked = folding()
	run:start({ input = "go", effort = "high" })
	t.equal(run:view().status.text, "watching the window")
	local rewrite = run:rewrite_context(view(first_run(), { tokens = 5010 })).rewrite
	-- The one request for a line: the result, whole.
	t.equal(#asked, 1)
	t.equal(asked[1].instructions, build.COMPACT_PROMPT)
	t.equal(asked[1].effort, "high")
	local wanted = "echo: [read] " .. contents("src/lib.rs")
	t.equal(string.sub(asked[1].task[2], -#wanted), wanted)
	-- The view, then the kept message.
	t.equal(rewrite.keep, { 4 })
	t.equal(rewrite.insert[1].before, 4)
	local lines = lines_of(rewrite.insert[1].text)
	t.equal(lines[1], "0+1|user: go")
	t.equal(lines[2], '1+1|tool: read {"path":"src/lib.rs"}')
	t.equal(string.sub(lines[3], 1, 20), "2+1|echo: read gave ")
	t.equal(#lines, 3)
	t.equal(rewrite.drops_conversation, true)
	t.equal(rewrite.details.entries, 3)
	t.equal(#rewrite.details.view, 3)
	t.equal(rewrite.details.tokens_before, 5010)
	t.equal(run.state.compacted.messages, 3)
	t.equal(run.state.compacted.asked, 1)
	t.equal(run:view().status.text, "3 messages in 3 lines")
end)

t.case("a second compaction folds only what came since", function()
	local run, asked = folding()
	local first = run:rewrite_context(view(first_run())).rewrite
	local second = {
		user(first.insert[1].text),
		user("and then this"),
		read("c2", "b.rs"),
		result("c2", "b.rs"),
		user("and also this"),
	}
	local rewrite = run:rewrite_context(view(second, { turn = 2 })).rewrite
	t.equal(string.sub(asked[2].task[1], 1, 27), "<chat>\nuser: go\ntool: read ")
	local labels = {}
	for _, line in ipairs(lines_of(rewrite.insert[1].text)) do
		table.insert(labels, string.split(line, "|")[1])
	end
	t.equal(labels, { "0+1", "1+1", "2+1", "3+1", "4+1", "5+1" })
	t.equal(lines_of(rewrite.insert[1].text)[4], "3+1|user: and then this")
	-- The second not counting the view it opened with.
	t.equal(run.state.compacted.messages, 3)
end)

t.case("an idle chat is the summary's", function()
	local run, asked = folding()
	t.equal(run:rewrite_context(view(first_run(), { trigger = "idle" })), nil)
	t.equal(#asked, 0)
end)

t.case("under the threshold, or with no window, nothing folds", function()
	local run, asked = folding()
	t.equal(run:rewrite_context(view(first_run(), { tokens = 900 })), nil)
	local unknown = tree.copy(ON)
	unknown.window = 0
	local open = folding({ settings = unknown })
	t.equal(open:rewrite_context(view(first_run(), { tokens = 10 ^ 9 })), nil)
	t.equal(#asked, 0)
end)

t.case("an overflow folds whatever the window, and fails with the line's error", function()
	local unknown = tree.copy(ON)
	unknown.window = 0
	local run, asked = folding({ settings = unknown })
	t.equal(run:rewrite_context(view(first_run(), { trigger = "overflow", tokens = 3 })) ~= nil, true)
	t.equal(#asked, 1)
	local failing = folding({
		answer = function()
			return { ok = false, error = "refused", reason = "error" }
		end,
	})
	local ok, why = pcall(function()
		return failing:rewrite_context(view(first_run(), { trigger = "overflow" }))
	end)
	t.equal(ok, false)
	t.equal(why, "the compactor failed on 2+1: refused")
end)

t.case("a failed line writes nothing, and is tried again after 2 turns", function()
	local run, asked = folding({
		answer = function(request, n)
			if n == 1 then
				return { ok = false, error = "refused", reason = "error" }
			end
			return { ok = true, value = "echo: a line" }
		end,
	})
	t.equal(run:rewrite_context(view(first_run(), { turn = 1 })), nil)
	t.equal(run.logs, { "the compactor failed on 2+1: refused" })
	t.equal(run.state.history, nil)
	t.equal(run:rewrite_context(view(first_run(), { turn = 2 })), nil)
	t.equal(#asked, 1)
	t.equal(run:rewrite_context(view(first_run(), { turn = 3 })) ~= nil, true)
	t.equal(run.state.failures, 0)
end)

t.case("zoom opens what was folded, and says when there is nothing", function()
	local run = folding()
	local ok, why = pcall(function()
		return run:tool("zoom", { id = 0, n = 1 })
	end)
	t.equal(ok, false)
	t.equal(why, "Nothing has been compacted yet: there is no line to open.")
	run:rewrite_context(view(first_run()))
	t.equal(run:tool("zoom", { id = 2, n = 1 }), "2+1|echo: [read] " .. contents("src/lib.rs"))
	t.equal(run:tool("zoom", { id = 0, n = 2 }), '0+1|user: go\n1+1|tool: read {"path":"src/lib.rs"}')
	ok, why = pcall(function()
		return run:tool("zoom", { id = 1, n = 2 })
	end)
	t.equal(why, "No line 1+2.")
end)

t.case("zoom is offered while the tree is on, and its pane switches it", function()
	local plugin = t.load()
	local zoom
	for _, tool in pairs(plugin.tools) do
		zoom = tool
	end
	t.equal(zoom.when, "on")
	local pane = folding():settings_view(ON)
	t.equal(pane.children[1].key, "on")
	t.equal(pane.children[1].label, "Compact into a tree")
end)

t.case("what it keeps reads back as the same history", function()
	local run = folding()
	run:rewrite_context(view(first_run()))
	local kept = run.state.history
	local history = tree.restore(kept.entries, kept.nodes, kept.view)
	t.equal(#history.entries, 3)
	t.equal(#history.view, 3)
end)
