-- tau-fast-compaction in a run: what it says as a run starts, what a pass
-- between turns drops and how the calls' cards show it, what an output
-- cut as it arrived puts on its card, and when a pass runs at all.
local tau = require("tau")
local t = tau.test
local g = t.gen
local output = require("output")

local function contains(text, part)
	return string.find(text, part, 1, true) ~= nil
end

local function starts_with(text, part)
	return string.sub(text, 1, #part) == part
end

local ARCHIVE = "/tmp/a/tau-"

-- A read of `id`, its tool named after it so a fake Jev can tell the
-- calls apart.
local function read(id)
	local args = { path = id .. ".rs" }
	return {
		role = "assistant",
		text = "",
		tokens = 1,
		calls = { { id = id, name = "tool_" .. id, args = args, order = { "path" }, json = json.encode(args) } },
	}
end

local function result(id, text)
	return { role = "toolResult", text = text or string.rep("line of " .. id .. "\n", 100), tokens = 1, call = id, name = "tool_" .. id, ok = true }
end

-- A prompt, a read and its result for each of `ids`, then a prompt: with
-- `preserve_recent = 1`, every call is judged.
local function messages_of(ids)
	local messages = { { role = "user", text = "fix the bug", tokens = 1 } }
	for _, id in ipairs(ids) do
		table.insert(messages, read(id))
		table.insert(messages, result(id))
	end
	table.insert(messages, { role = "user", text = "and then this", tokens = 1 })
	return messages
end

local function view_of(messages, spec)
	spec = spec or {}
	for at, message in ipairs(messages) do
		message.index = at
	end
	return {
		trigger = spec.trigger or "overflow",
		turn = 3,
		tokens = spec.tokens or 5000,
		window = spec.window,
		messages = messages,
	}
end

local SETTINGS = { preserve_recent = 1, window = 1000 }

-- A Jev that answers each call's questions by `actions[call id]`: keep,
-- keep only its call, or neither. Asks are counted in `asked.count`.
local function jev_for(actions, asked, fail)
	return {
		ask = function(req)
			asked.count += 1
			if fail then
				error("Jev answered with status 503", 0)
			end
			local answers = {}
			for id, question in pairs(req.questions) do
				local call_id = string.match(question.question, "%(tool_([%w%-]+)")
				local action = actions[call_id] or "keep"
				local p
				if string.sub(id, 1, 5) == "call_" then
					p = action == "drop_call" and 0.1 or 0.9
				else
					p = action == "keep" and 0.9 or 0.1
				end
				answers[id] = { probability = p }
			end
			return answers, 0.001
		end,
	}
end

-- A fake run with Jev answering by `actions`, whose rewrites save half
-- the transcript unless `saved` says otherwise, and that keeps the
-- archives it writes in `written`.
local function running(actions, spec)
	spec = spec or {}
	local asked = { count = 0 }
	local written = {}
	local made = 0
	local jev = nil
	if not spec.no_jev then
		jev = jev_for(actions, asked, spec.fail)
	end
	local run = t.run({
		settings = spec.settings or SETTINGS,
		state = spec.state,
		jev = jev,
		transcript = spec.transcript,
		tools = {
			measure = function(rewrite)
				return { before = 1000, after = 1000 - (spec.saved or 500) }
			end,
			archive_path = function(a)
				made += 1
				return ARCHIVE .. a.kind .. "-" .. made .. ".txt"
			end,
			archive_write = function(a)
				written[a.path] = a.text
			end,
		},
	})
	return run, asked, written
end

-- As a run starts it says whether it prunes: only with a TypeSafe key.
t.case("it says whether it is on", function()
	local on = running({})
	on:start({ input = "go" })
	t.equal(on.state.on, true)
	t.equal(on:view().status.text, "pruning large outputs · watching the window")
	local off = running({}, { no_jev = true })
	off:start({ input = "go" })
	t.equal(off.state.on, false)
	t.equal(off:view().status.text, "off · no TypeSafe key")
	-- Without a key nothing is pruned, whatever the trigger.
	t.equal(off:rewrite_context(view_of(messages_of({ "a" }))), nil)
	local big = string.rep("step of the build\n", 4000)
	t.equal(off:after_tool_result({ id = "x", name = "exec_command", args = { cmd = "make" }, ok = true, text = big }), nil)
end)

-- A pass drops a call Jev says is done with, cuts the result of one
-- whose call matters but not its output, keeping that result whole in
-- an archive, and keeps the rest; the cards say so, and the status says
-- how many it pruned and by how much.
t.case("a pass marks what it drops on the calls' cards", function()
	local run, asked, written = running({ a = "keep", b = "drop_result", c = "drop_call" })
	local messages = messages_of({ "a", "b", "c" })
	local rewrite = run:rewrite_context(view_of(messages))
	t.equal(asked.count, 1)
	t.equal(rewrite.rewrite.drop_calls, { "c" })
	t.equal(#rewrite.rewrite.texts, 1)
	local cut = rewrite.rewrite.texts[1]
	t.equal(cut.index, 5)
	local archive = ARCHIVE .. "result-1.txt"
	t.equal(contains(cut.text, "; full result: " .. archive .. " (read or grep it if needed)]"), true, cut.text)
	t.equal(written[archive], messages[5].text)
	t.equal(#rewrite.rewrite.keep, #messages)
	local view = run:view()
	t.equal(view.status.text, "2 pruned · −50%")
	t.equal(view.calls.a, nil)
	t.equal(view.calls.b.badge.text, "result dropped")
	t.equal(view.calls.b.dropped, "result")
	t.equal(view.calls.c.badge.text, "call dropped")
	t.equal(view.calls.c.dropped, "call")
	t.equal(#run.state.decisions, 3)
	t.equal(rewrite.rewrite.details.stats.calls_dropped, 1)
	t.equal(rewrite.rewrite.details.stats.results_dropped, 1)
end)

-- Every decision has its card label: pinned and kept calls show none.
t.case("each decision has its card label", function()
	local run = running({}, {
		state = {
			passes = {
				{ before = 100, after = 50, results_dropped = 1, calls_dropped = 1, reduction_ratio = 0.5, detail = "d", window = 1000 },
			},
			ledger = {
				{ call_id = "p", tool = "read", input = "", tokens = 1, matters = json.null, verbatim = json.null, decision = "pinned" },
				{ call_id = "k", tool = "read", input = "", tokens = 1, matters = 0.9, verbatim = 0.9, decision = "keep" },
				{ call_id = "r", tool = "read", input = "", tokens = 1, matters = 0.9, verbatim = 0.1, decision = "drop_result" },
				{ call_id = "d", tool = "read", input = "", tokens = 1, matters = 0.1, verbatim = 0.1, decision = "drop_call" },
			},
		},
	})
	local calls = run:view().calls
	t.equal(calls.p, nil)
	t.equal(calls.k, nil)
	t.equal(calls.r.badge.text, "result dropped")
	t.equal(calls.d.badge.text, "call dropped")
end)

local RANK = { keep = 1, drop_result = 2, drop_call = 3 }
local ACTIONS = { "keep", "drop_result", "drop_call" }

-- `messages` as the host leaves them after `rewrite`: dropped calls gone
-- with their results, cut results replaced.
local function applied(messages, rewrite)
	local dropped, texts = {}, {}
	for _, id in ipairs(rewrite.drop_calls) do
		dropped[id] = true
	end
	for _, change in ipairs(rewrite.texts) do
		texts[change.index] = change.text
	end
	local out = {}
	for index, message in ipairs(messages) do
		local id = message.call or (message.calls and message.calls[1] and message.calls[1].id)
		if not (id and dropped[id]) then
			local made = table.clone(message)
			made.text = texts[index] or message.text
			table.insert(out, made)
		end
	end
	return out
end

-- Over any passes, each marks on the cards what the ledger has made of
-- every call so far: a decision only escalates, a call dropped by an
-- earlier pass stays on the ledger and its card, and a kept one has no
-- mark. The ledger lists every call ever judged.
t.property("passes mark what they drop", function(draw)
	local count = draw(g.integers({ min = 1, max = 6 }))
	local ids = {}
	for at = 1, count do
		ids[at] = "c" .. at
	end
	local passes = draw(g.lists(g.lists(g.sampled_from(ACTIONS), { min = count, max = count }), { min = 1, max = 4 }))
	local actions = {}
	local run = running(actions)
	local messages = messages_of(ids)
	local held = {}
	for _, drawn in ipairs(passes) do
		for at, id in ipairs(ids) do
			actions[id] = drawn[at]
		end
		local present = {}
		for _, message in ipairs(messages) do
			if message.calls and message.calls[1] then
				present[message.calls[1].id] = true
			end
		end
		local rewrite = run:rewrite_context(view_of(messages))
		if rewrite ~= nil then
			for at, id in ipairs(ids) do
				if present[id] and (held[id] == nil or RANK[drawn[at]] > RANK[held[id]]) then
					held[id] = drawn[at]
				end
			end
			messages = applied(messages, rewrite.rewrite)
		end
	end
	local calls = run:view().calls
	for _, id in ipairs(ids) do
		local decision = held[id]
		if decision == "drop_result" then
			t.equal(calls[id].badge.text, "result dropped", id)
			t.equal(calls[id].dropped, "result", id)
		elseif decision == "drop_call" then
			t.equal(calls[id].badge.text, "call dropped", id)
			t.equal(calls[id].dropped, "call", id)
		else
			t.equal(calls[id], nil, id)
		end
	end
	t.equal(#run.state.ledger, count)
	for _, decision in ipairs(run.state.decisions) do
		t.equal(decision.action, held[decision.call_id], decision.call_id)
	end
end, { cases = 100 })

-- 400 lines of a build: large enough for the gate at 100 tokens.
local BUILD = (function()
	local steps = {}
	for n = 0, 399 do
		table.insert(steps, "step " .. n .. " of the build")
	end
	return table.concat(steps, "\n")
end)()

local OUTPUT_SETTINGS = { output_min_tokens = 100 }

-- A Jev that keeps only the output's second chunk, or every chunk.
local function chunk_jev(keep_all, asked)
	return {
		ask = function(req)
			asked.count += 1
			local answers = {}
			for id in pairs(req.questions) do
				answers[id] = { probability = (keep_all or id == "c2") and 0.9 or 0 }
			end
			return answers, 0.002
		end,
	}
end

local function output_run(keep_all)
	local asked = { count = 0 }
	local written = {}
	local run = t.run({
		settings = OUTPUT_SETTINGS,
		jev = chunk_jev(keep_all, asked),
		transcript = { { index = 1, role = "user", text = "build it" } },
		tools = {
			archive_path = function(a)
				return ARCHIVE .. a.kind .. "-1.txt"
			end,
			archive_write = function(a)
				written[a.path] = a.text
			end,
		},
	})
	return run, asked, written
end

-- An output cut as it arrived: the model reads the kept chunks, the
-- whole output goes to its archive, and its card says what was kept of
-- it and where the rest is.
t.case("a cut output goes on its card", function()
	local run, asked, written = output_run(false)
	local answer = run:after_tool_result({ id = "x", name = "exec_command", args = { cmd = "make" }, ok = true, text = BUILD, truncated = false })
	t.equal(asked.count >= 1, true)
	t.equal(answer.flag, false)
	t.equal(starts_with(answer.text, output.HEADER .. "\n"), true)
	t.equal(contains(answer.text, "step 0 of the build\n"), true)
	t.equal(contains(answer.text, "step 39 of the build\n"), true)
	t.equal(contains(answer.text, "step 40 of the build"), false)
	t.equal(contains(answer.text, "step 399 of the build"), true)
	t.equal(contains(answer.text, "[340 lines omitted]"), true)
	local archive = ARCHIVE .. "output-1.txt"
	t.equal(written[archive], BUILD)
	local cut = run:view().calls.x.cut
	t.equal(cut.kept, 60)
	t.equal(cut.lines, 400)
	t.equal(cut.archive, archive)
	t.equal(cut.tokens_after < cut.tokens_before, true)
	t.equal(cut.cost, 0.002 * asked.count)
end)

-- An output Jev keeps whole stays as it was, writes no archive and
-- puts nothing on its card; one the gate stops never reaches Jev.
t.case("an output kept whole or gated changes nothing", function()
	local run, asked, written = output_run(true)
	t.equal(run:after_tool_result({ id = "x", name = "exec_command", args = { cmd = "make" }, ok = true, text = BUILD }), nil)
	t.equal(asked.count >= 1, true)
	t.equal(next(written), nil)
	t.equal(run.state.outputs[1].pruned, false)
	t.equal(run:view().calls.x, nil)
	local gated, never = output_run(false)
	t.equal(gated:after_tool_result({ id = "y", name = "read", args = { path = "a" }, ok = true, text = BUILD }), nil)
	t.equal(gated:after_tool_result({ id = "z", name = "exec_command", args = { cmd = "make" }, ok = false, text = BUILD }), nil)
	t.equal(gated:after_tool_result({ id = "w", name = "exec_command", args = { cmd = "make" }, ok = true, text = BUILD, truncated = true }), nil)
	t.equal(never.count, 0)
end)

-- Between turns a pass runs past the share of the window, and then only
-- once the context has grown by the cooldown since the last one.
t.case("the cooldown and the share of the window gate a pass", function()
	local run, asked = running({ a = "drop_result" })
	local function at(tokens)
		return run:rewrite_context(view_of(messages_of({ "a", "b" }), { trigger = "turn_end", tokens = tokens }))
	end
	-- 60% of the window of 1000 by default.
	t.equal(at(500), nil)
	t.equal(asked.count, 0)
	t.equal(at(700) ~= nil, true)
	t.equal(asked.count, 1)
	-- Within the cooldown of 10%.
	t.equal(at(750), nil)
	t.equal(asked.count, 1)
	t.equal(at(800) ~= nil, true)
	t.equal(asked.count, 2)
	-- No window, no pass.
	local open, never = running({}, { settings = { preserve_recent = 1 } })
	t.equal(open:rewrite_context(view_of(messages_of({ "a" }), { trigger = "turn_end", tokens = 10 ^ 9 })), nil)
	t.equal(never.count, 0)
end)

-- An overflow runs a pass whatever the window says, and fails with the
-- pass's error; a failure between turns is logged and changes nothing.
t.case("overflow always runs", function()
	local run, asked = running({ a = "drop_result" })
	t.equal(run:rewrite_context(view_of(messages_of({ "a" }), { tokens = 1 })) ~= nil, true)
	t.equal(asked.count, 1)
	local failing = running({}, { fail = true })
	local ok, why = pcall(function()
		return failing:rewrite_context(view_of(messages_of({ "a" })))
	end)
	t.equal(ok, false)
	t.equal(contains(tostring(why), "Jev answered with status 503"), true, tostring(why))
	local quiet = running({}, { fail = true })
	t.equal(quiet:rewrite_context(view_of(messages_of({ "a" }), { trigger = "turn_end", tokens = 900 })), nil)
	t.equal(#quiet.logs, 1)
	t.equal(quiet.state.decisions, nil)
end)

-- A run starting from a fork's transcript is rewritten only when it
-- would not fit the window with the start reserve.
t.case("a start rewrites only a transcript too full for the window", function()
	local settings = { preserve_recent = 1, window = 100000 }
	local run, asked = running({ a = "drop_result" }, { settings = settings })
	t.equal(run:rewrite_context(view_of(messages_of({ "a" }), { trigger = "start", tokens = 50000 })), nil)
	t.equal(asked.count, 0)
	t.equal(run:rewrite_context(view_of(messages_of({ "a" }), { trigger = "start", tokens = 90000 })) ~= nil, true)
	t.equal(asked.count, 1)
end)

-- An idle run is never pruned, however full.
t.case("idle never does", function()
	local run, asked = running({ a = "drop_call" })
	t.equal(run:rewrite_context(view_of(messages_of({ "a" }), { trigger = "idle", tokens = 10 ^ 6 })), nil)
	t.equal(asked.count, 0)
end)

-- A pass whose rewrite would save too little leaves the transcript, the
-- ledger and the archives as they were.
t.case("too small a saving declines without changing the ledger", function()
	local before = { { call_id = "z", tool = "read", action = "drop_call", keep_call = 0.1, keep_result = 0.1 } }
	local run, asked, written = running({ a = "drop_result", b = "drop_call" }, { saved = 100, state = { decisions = before } })
	t.equal(run:rewrite_context(view_of(messages_of({ "a", "b" }))), nil)
	t.equal(asked.count, 1)
	t.equal(run.state.decisions, before)
	t.equal(run.state.passes, nil)
	t.equal(next(written), nil)
end)
