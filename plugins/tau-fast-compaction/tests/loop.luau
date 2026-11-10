-- tau-fast-compaction's history pruning in a real agent loop (ADR 0053):
-- a scripted model, a fake `read` whose output names the file it read and
-- is `size` characters long, a fake Jev whose answers each loop chooses,
-- an in-memory store and real time.
local tau = require("tau")
local t = tau.test

local function contains(text, part)
	return string.find(text, part, 1, true) ~= nil
end

local function starts_with(text, part)
	return string.sub(text, 1, #part) == part
end

-- What a fake `read` of `path`, `size` characters long, answers.
local function contents(path, size)
	return "CONTENTS OF " .. path .. ": " .. string.rep("x", size)
end

local TOOLS = { read = "CONTENTS OF {path}: {x*size}" }

-- Settings for a 1,000-token window, pinning only the last message, with
-- `more` over them.
local function settings(more)
	local out = { window = 1000, preserve_recent = 1 }
	for key, value in pairs(more or {}) do
		out[key] = value
	end
	return out
end

-- A turn reading `path`, `size` characters, its usage `input` tokens.
local function read(path, size, input)
	local call = { tool = "read", args = { path = path, size = size } }
	if input == nil then
		return call
	end
	return { call, { usage = { input = input, output = 10 } } }
end

-- Two reads, the second pushing the context past 60% of the window,
-- then an answer.
local function two_reads()
	return { read("a.rs", 4000, 100), read("b.rs", 10, 700), { text = "done" } }
end

-- A run of `model`, its other fields from `more`.
local function spec(model, more)
	local out = { prompt = "fix the bug in a.rs", tools = TOOLS, model = model }
	for key, value in pairs(more or {}) do
		out[key] = value
	end
	return out
end

-- The text of a tool result message.
local function result_text(message)
	t.equal(message.role, "toolResult", "a tool result")
	return message.content[1].text
end

-- Each `ContextRewritten` event: the plugin, the tokens before and after.
local function rewritten(events)
	local out = {}
	for _, event in ipairs(events) do
		local rewrite = event.ContextRewritten
		if rewrite then
			table.insert(out, { plugin = rewrite.plugin, before = rewrite.tokens_before, after = rewrite.tokens_after })
		end
	end
	return out
end

-- Past 60% of the window, the plugin asks Jev about the one unpinned
-- call, and on "the call matters, its output does not", cuts the output
-- to its head and a note. The next request sends the pruned transcript;
-- the store keeps the rewrite and its ledger; Jev's usage is charged to
-- the run; Jev saw the call, never its output.
t.loop(
	"a stale result is cut",
	spec(two_reads(), {
		settings = settings(),
		jev = { { id = "call_", noul = 0.9 }, { noul = 0.1 } },
	}),
	function(run)
		t.equal(run.outcome.stop, "Stop")
		t.equal(run.outcome.text, "done")

		local rewrites = rewritten(run.events)
		t.equal(#rewrites, 1, json.encode(rewrites))
		t.equal(rewrites[1].plugin, "tau-luau-plugins")
		-- The cut result's size is gone from the context, though the last
		-- call's usage still counts it.
		t.equal(rewrites[1].after + 500 < rewrites[1].before, true, json.encode(rewrites))

		-- What Jev cost for the pass is what the run was charged.
		t.equal(run.state.passes[1].results_dropped, 1)
		t.equal(run.charged > 0, true, "charged")
		local next = run.requests[3].messages
		local first = result_text(next[3])
		t.equal(starts_with(first, "CONTENTS OF a.rs: "), true, first)
		t.equal(contains(first, "[fast-compaction truncated") and #first < 500, true, first)
		-- The note names an archive holding the result whole.
		local archive = string.match(first, "; full result: (.-) %(read or grep it if needed%)%]$")
		t.equal(archive ~= nil, true, first)
		t.equal(run.archive[archive] and run.archive[archive].text, contents("a.rs", 4000), archive)
		t.equal(result_text(next[5]), contents("b.rs", 10))

		t.equal(#run.jev, 1)
		t.equal(run.jev[1].questions, { "call_t1", "result_t1" })
		local state = json.encode(run.jev[1].state)
		t.equal(contains(state, string.rep("x", 100)), false, "the output reached Jev")
		t.equal(contains(state, "a.rs"), true, state)

		t.equal(#run.rewrites, 1)
		t.equal(run.rewrites[1].plugin, "tau-fast-compaction")
		local details = run.rewrites[1].details
		t.equal(details.stats.results_dropped, 1)
		t.equal(details.stats.reduction_ratio >= 0.25, true, tostring(details.stats.reduction_ratio))
		t.equal(details.stats.cost, run.charged)
		t.equal(run.outcome.usage.cost.total > 0, true, "Jev's usage is charged")
	end
)

-- On "neither the call nor its output matters", the call goes with its
-- result.
t.loop(
	"a stale call is dropped",
	spec(two_reads(), { settings = settings(), jev = { { noul = 0 } } }),
	function(run)
		t.equal(run.outcome.text, "done")
		local next = run.requests[3].messages
		t.equal(#next, 3, "the first call and its result are gone")
		t.equal(result_text(next[3]), contents("b.rs", 10))
	end
)

-- A pass that would save too little declines: nothing is rewritten, and
-- the cooldown keeps the next turns from asking again. The cooldown is a
-- share of the window: on a 100,000-token window, growing 9,000 tokens
-- after a pass does not run another, and growing past 10,000 does. The
-- first turn's one call is pinned, so nothing is asked there.
t.loop(
	"the cooldown grows with the window",
	spec({
		read("a.rs", 10, 100),
		read("b.rs", 10, 61000),
		read("c.rs", 10, 70000),
		read("d.rs", 10, 72000),
		{ text = "done" },
	}, {
		settings = settings({ window = 100000, min_reduction_ratio = 0.9 }),
		jev = { { noul = 0 } },
	}),
	function(run)
		t.equal(run.outcome.text, "done")
		t.equal(#rewritten(run.events), 0, "each pass saves too little")
		t.equal(#run.jev, 2, "passes at 61k and 72k only")
	end
)

-- An overflow runs a pass whatever the window share, and the turn is
-- retried on the pruned transcript.
t.loop(
	"an overflow prunes and retries",
	spec({
		read("a.rs", 4000),
		read("b.rs", 10),
		{ error = { code = "context_length_exceeded", message = "too long" } },
		{ text = "done" },
	}, {
		settings = settings({ window = 1000000000000 }),
		jev = { { noul = 0 } },
	}),
	function(run)
		t.equal(run.outcome.text, "done")
		-- Every turn the model was given was asked for, and no more.
		t.equal(#run.requests, 4)
		t.equal(#rewritten(run.events), 1)
		t.equal(#run.requests[4].messages, 3)
	end
)

-- A Jev failure is in the plugin's log and leaves the run going,
-- uncompacted.
t.loop(
	"a jev failure is reported",
	spec(two_reads(), { settings = settings(), jev = { error = 503 } }),
	function(run)
		t.equal(run.outcome.text, "done")
		t.equal(#run.logs, 1, json.encode(run.logs))
		t.equal(contains(run.logs[1], "Jev answered with status 503"), true, run.logs[1])
		t.equal(#run.requests[3].messages, 5)
	end
)

-- A fork of a pruned run starts from the pruned transcript, and the
-- plugin resumes its ledger from the stored rewrite: a later pass in the
-- fork, told the old result now matters, still holds its cut, since
-- decisions only escalate. Started afresh, the fork's ledger would hold
-- the milder answer.
local turns = two_reads()
-- The fork's turns: c.rs is the last call, and kept, at the first pass;
-- the second, past the cooldown, may judge it.
table.insert(turns, read("c.rs", 4000, 100))
table.insert(turns, read("d.rs", 10, 1300))
table.insert(turns, { text = "forked" })
t.loop(
	"a fork resumes the ledger",
	spec(turns, {
		settings = settings({ start_reserve_tokens = 0 }),
		-- Before the fork, a.rs's output does not matter; after it, it
		-- does, and c.rs's does not.
		jev = { { id = "result_t1", noul = 0.1 }, { id = "result_t3", noul = 0.1 }, { noul = 0.9 } },
		["then"] = {
			{ prompt = "and now", jev = { { id = "result_t3", noul = 0.1 }, { noul = 0.9 } } },
		},
	}),
	function(run)
		local fork = run.forks[1]
		t.equal(fork.outcome.text, "forked")
		local seen = run.requests[4].messages
		t.equal(contains(result_text(seen[3]), "[fast-compaction truncated"), true, result_text(seen[3]))

		local details = fork.rewrites[#fork.rewrites].details
		local function action(call)
			for _, decision in ipairs(details.decisions) do
				if decision.call_id == call then
					return decision.action
				end
			end
			return nil
		end
		-- Jev's questions name calls `t1`, `t2`… in order; the ledger
		-- names them by their ids.
		t.equal(action("call_2|fc_2"), "drop_result", "c.rs")
		t.equal(action("call_0|fc_0"), "drop_result", "a.rs")
	end
)

-- A fork starts on its parent's transcript past 60% of the window
-- without a pass, which would give up the parent's prompt cache; one
-- within the start reserve of the window gets a pass first. About 6,000
-- tokens of output in a 10,000-token window: the scripted model adds the
-- new output to what it reports.
for _, case in ipairs({ { reserve = 1000, asked = 0 }, { reserve = 5000, asked = 1 } }) do
	local function at(more)
		return settings({ window = 10000, start_reserve_tokens = case.reserve, compact_at_percent = more })
	end
	t.loop(
		"a fork prunes at start only when it must, reserve " .. case.reserve,
		spec({
			read("a.rs", 24000, 100),
			{ { text = "done" }, { usage = { input = 100, output = 10 } } },
			{ text = "forked" },
		}, {
			-- The parent never prunes.
			settings = at(100),
			jev = { { noul = 0.1 } },
			["then"] = { { prompt = "and now", settings = at(nil) } },
		}),
		function(run)
			-- The parent asked Jev nothing: each answer is charged.
			t.equal(run.charged, 0, "the parent asked Jev")
			t.equal(run.forks[1].outcome.text, "forked")
			t.equal(#run.jev, case.asked, "reserve " .. case.reserve)
		end
	)
end
