-- tau-reasoning in a run (docs/reference/plugins.md): a confident score
-- sets the effort, an unsure one keeps the last, a chosen effort stands,
-- a failure says so, a lease says when Jev picks again, and a long
-- conversation keeps its effort for the cache.
local tau = require("tau")
local t = tau.test
local g = t.gen
local effort = require("effort")

local EFFORTS = { "low", "medium", "high" }

-- A Jev that scores `probabilities` (one per level) at `confidence`,
-- leases `lease`, and keeps every request in `asked`.
local function jev(probabilities, confidence, lease)
	local asked = {}
	local fake = {
		ask = function(request)
			table.insert(asked, request)
			local p = {}
			for n, value in ipairs(probabilities) do
				p[tostring(n - 1)] = value
			end
			local answers = { effort = { score = 0, confidence = confidence, probabilities = p } }
			if request.questions.lease then
				answers.lease = { choice = lease or "tool_chain", confidence = 1, probabilities = {} }
			end
			return answers, 0.001
		end,
	}
	return fake, asked
end

local function run(spec)
	return t.run({
		id = spec.id or "r1",
		settings = spec.settings or { redecide = false, threshold = 0.7 },
		state = spec.state,
		jev = spec.jev,
		transcript = spec.transcript,
	})
end

local function starting(spec)
	return {
		input = spec.input or "fix the flaky test",
		instructions = spec.instructions or "You are a careful coding agent.",
		model = "m",
		efforts = spec.efforts or EFFORTS,
		effort = spec.effort,
		prefix_tokens = spec.prefix or 100,
	}
end

t.case("a confident score sets the effort", function()
	local fake = jev({ 0.1, 0.1, 0.8 }, 0.9)
	local r = run({ jev = fake })
	t.equal(r:start(starting({})), tau.start({ effort = "high" }))
	local view = r:view()
	t.equal(view.plan, { name = "reasoning", value = "high" })
	t.equal(view.status.text, "chose high")
	t.equal(r.state.notes.n0.text, "picked **high** reasoning for this message")
end)

-- The status and plan are a message's: the next message, with an effort
-- the person set, shows that, not the effort the last one chose.
t.case("a message's choice does not stay on the next message", function()
	local r = run({ jev = jev({ 0.1, 0.1, 0.8 }, 0.9) })
	r:start(starting({}))
	t.equal(r:view().status.text, "chose high")
	r:start(starting({ effort = "low" }))
	local view = r:view()
	t.equal(view.status.text, "off · effort set to low")
	t.equal(view.plan, nil)
end)

t.case("an unsure score keeps the default", function()
	local r = run({ jev = jev({ 0.2, 0.3, 0.5 }, 0.4) })
	t.equal(r:start(starting({})), nil)
	t.equal(r:view().status.text, "kept the default")
	t.equal(r.state.choices[1].verdict, "kept")
end)

t.case("the levels are the model's efforts", function()
	local fake, asked = jev({ 0.5, 0.5 }, 0.9)
	run({ jev = fake }):start(starting({ efforts = { "minimal", "high" } }))
	t.equal(#asked[1].questions.effort.levels, 2)
	t.equal(string.find(asked[1].questions.effort.levels[1], "lookups", 1, true) ~= nil, true)
end)

t.case("a chosen effort stands, a model with no effort to choose is not scored, and no key is off", function()
	local fake, asked = jev({ 1 }, 1)
	local chosen = run({ jev = fake })
	t.equal(chosen:start(starting({ effort = "low" })), nil)
	t.equal(chosen:view().status.text, "off · effort set to low")
	run({ jev = fake }):start(starting({ efforts = {} }))
	run({ jev = fake }):start(starting({ efforts = { "high" } }))
	t.equal(#asked, 0)
	local keyless = run({})
	keyless:start(starting({}))
	t.equal(keyless:view().status.text, "off · no TypeSafe key")
end)

t.case("a failed request keeps the default and says so", function()
	local failing = { ask = function() error("Jev is down", 0) end }
	local r = run({ jev = failing })
	t.equal(r:start(starting({})), nil)
	t.equal(r.state.notes.n0.kind, "failed")
	t.equal(string.find(r.state.notes.n0.message, "Jev could not score the task: Jev is down", 1, true) ~= nil, true)
	t.equal(r.state.notes.n0.detail, "kept the default")
end)

t.case("an unsure message goes on as the last one ran", function()
	local first = run({ jev = jev({ 0, 0, 1 }, 0.9) })
	first:start(starting({}))
	first:run_end({ stop = "Stop", turns = 1, cost = 0, text = "Done." })
	local second = run({ id = "r2", state = first.state, jev = jev({ 1, 0, 0 }, 0.3) })
	t.equal(second:start(starting({ input = "and the other one" })), tau.start({ effort = "high" }))
	t.equal(second:view().status.text, "stayed at high")
end)

t.case("a short ask brings the task it answers", function()
	local first = run({ jev = jev({ 0, 1, 0 }, 0.9) })
	first:start(starting({ input = "refactor the store layer" }))
	first:run_end({ stop = "Stop", turns = 1, cost = 0, text = "Shall I also move the tests?" })
	local fake, asked = jev({ 0, 1, 0 }, 0.9)
	run({ id = "r2", state = first.state, jev = fake }):start(starting({ input = "yes, do it" }))
	t.equal(asked[1].state.previous_task, "refactor the store layer")
	t.equal(asked[1].state.last_proposal, "Shall I also move the tests?")
end)

t.case("a long conversation keeps its effort for the cache", function()
	local first = run({ jev = jev({ 0, 1, 0 }, 0.9) })
	first:start(starting({}))
	first:run_end({ stop = "Stop", turns = 1, cost = 0, text = "Done." })
	local second = run({ id = "r2", state = first.state, jev = jev({ 0, 0, 1 }, 0.95) })
	t.equal(second:start(starting({ prefix = 48000 })), tau.start({ effort = "medium" }))
	t.equal(second:view().status.text, "kept medium")
	local entry = second.state.notes.n1
	t.equal(entry.text, "kept **medium** reasoning for this message")
	t.equal(string.find(entry.outcome, "would resend 48k tokens uncached", 1, true) ~= nil, true, entry.outcome)
end)

local TOOLS = {
	{ index = 1, role = "user", text = "fix it" },
	{ index = 2, role = "assistant", text = "Running the tests.", calls = { { id = "c1", name = "exec_command", args = {}, order = {} } } },
	{ index = 3, role = "toolResult", text = "ok", call = "c1", name = "exec_command", ok = true },
}

t.case("by default the effort holds for the run", function()
	local fake, asked = jev({ 0, 1, 0 }, 0.9)
	local r = run({ jev = fake, transcript = TOOLS })
	r:start(starting({}))
	r:before_request({ effort = "medium", turn = 1, replies = 0 })
	r:before_request({ effort = "medium", turn = 2, replies = 1 })
	t.equal(#asked, 1)
end)

t.case("a lease says when Jev picks again, and a failed tool ends it", function()
	local fake, asked = jev({ 0, 1, 0 }, 0.9, "one_call")
	local r = run({ jev = fake, transcript = TOOLS, settings = { redecide = true, threshold = 0.7 } })
	r:start(starting({}))
	t.equal(asked[1].questions.lease ~= nil, true)
	r:before_request({ effort = "medium", turn = 1, replies = 0 })
	t.equal(#asked, 1, "the first request is the one start chose for")
	r:before_request({ effort = "medium", turn = 2, replies = 1 })
	t.equal(#asked, 2, "a one-call lease ended")
	t.equal(asked[2].state.step, "tool_step")
	t.equal(asked[2].state.tool_results.calls, 1)
	local holding, held = jev({ 0, 1, 0 }, 0.9, "user_turn")
	local failed = {
		TOOLS[1],
		TOOLS[2],
		{ index = 3, role = "toolResult", text = "boom", call = "c1", name = "exec_command", ok = false },
	}
	local again = run({ jev = holding, transcript = failed, settings = { redecide = true, threshold = 0.7 } })
	again:start(starting({}))
	again:before_request({ effort = "medium", turn = 1, replies = 0 })
	again:before_request({ effort = "medium", turn = 2, replies = 1 })
	t.equal(#held, 2, "a failed tool ends even a user-turn lease")
end)

-- Notes show only where the effort changes, whatever the choices.
t.property("notes show only where the effort changes", function(draw)
	local state = effort.state({})
	local before = nil
	local notes = 0
	for _ = 1, draw(g.integers({ min = 1, max = 8 })) do
		local runs_at = draw(g.optional(g.sampled_from(EFFORTS)))
		local chose = draw(g.booleans())
		local choice = {
			verdict = chose and "chose" or "kept",
			effort = runs_at or "low",
			confidence = 0.8,
			threshold = 0.7,
			levels = {},
			cost = 0,
			step = "user_turn",
			runs_at = runs_at,
		}
		local key = effort.record_choice(state, choice)
		local ran = effort.ran_at(choice)
		t.equal(key ~= nil, ran ~= before, "a note where it changed")
		if key then
			notes += 1
		end
		before = ran
	end
	local count = 0
	for _ in pairs(state.notes) do
		count += 1
	end
	t.equal(count, notes)
end)

t.property("token counts read in thousands from a thousand", function(draw)
	local n = draw(g.integers({ min = 0, max = 10000000 }))
	local text = effort.tokens_text(n)
	if n < 1000 then
		t.equal(text, tostring(n))
	else
		t.equal(text, ((n + 500) // 1000) .. "k")
	end
end)

local function chosen(chose, level, runs_at)
	return {
		verdict = chose and "chose" or "kept",
		effort = level,
		confidence = 0.8,
		threshold = 0.7,
		levels = {},
		cost = 0,
		step = "user_turn",
		runs_at = runs_at,
	}
end

-- The text of each note `choices` leave, in order.
local function texts(choices)
	local state = effort.state({})
	local out = {}
	for _, choice in ipairs(choices) do
		local key = effort.record_choice(state, choice)
		if key then
			table.insert(out, state.notes[key].text)
		end
	end
	return out, state
end

t.case("a note says what changed", function()
	t.equal(
		texts({
			chosen(false, "low", nil),
			chosen(true, "high", "high"),
			chosen(false, "low", "high"),
			chosen(true, "low", "low"),
		}),
		{ "picked **high** reasoning for this message", "reasoning **high** → **low**" }
	)
end)

t.case("a kept effort says what it saved", function()
	local kept = chosen(true, "high", "medium")
	kept.kept_for_cache = 48213
	local notes, state = texts({ chosen(true, "medium", "medium"), kept })
	t.equal(notes, { "picked **medium** reasoning for this message", "kept **medium** reasoning for this message" })
	t.equal(
		state.notes.n1.outcome,
		"but this message stays at medium: switching to high would resend 48k tokens uncached."
	)
	t.equal(state.status, "kept medium")
end)

-- The sticky-effort rule, as the plugin's docs say it, by cases: a
-- reference for `effort.runs_at`.
local function reference(picked, known, last, prefix, sticky_after)
	if picked == nil then
		-- Unsure or failed: as the last message ran, or the default.
		return last, nil
	end
	if not known then
		-- Nothing ran yet: nothing to keep.
		return picked, nil
	end
	if prefix < sticky_after or last == picked then
		return picked, nil
	end
	return last, prefix
end

local ALL = { "none", "minimal", "low", "medium", "high", "xhigh", "max" }

-- For any pick, last effort, prefix and threshold, the message runs as
-- the reference says; what it runs at is Jev's pick or the last effort,
-- and only a kept effort says how many tokens it kept.
t.property("the sticky effort follows the rule", function(draw)
	local picked = draw(g.optional(g.sampled_from(ALL)))
	local known = draw(g.booleans())
	local last = known and draw(g.optional(g.sampled_from(ALL))) or nil
	local sticky_after = draw(g.integers({ min = 0, max = 100000 }))
	-- The threshold itself and just under it, as often as anything else.
	local prefix = ({ sticky_after, math.max(sticky_after - 1, 0) })[draw(g.integers({ min = 1, max = 3 }))]
		or draw(g.integers({ min = 0, max = 200000 }))
	local runs, kept = effort.runs_at(picked, known, last, prefix, sticky_after)
	local want_runs, want_kept = reference(picked, known, last, prefix, sticky_after)
	t.equal({ runs, kept }, { want_runs, want_kept })
	t.equal(runs == picked or runs == last, true)
	if kept ~= nil then
		t.equal(kept, prefix)
		t.equal(prefix >= sticky_after, true)
		t.equal(runs ~= picked, true)
	end
end)

-- A Jev sure of `level` (by its place among EFFORTS, from 1).
local function sure(level)
	local p = {}
	for n = 1, #EFFORTS do
		p[n] = n == level and 1 or 0
	end
	return jev(p, 0.9)
end

-- A message after a first one, Jev's pick or one the person chose, keeps
-- the effort the conversation last ran at once its prefix reaches the
-- threshold, and says how many tokens that kept; under it, Jev's pick
-- runs.
t.property("a long conversation keeps its effort", function(draw)
	local chose = draw(g.optional(g.sampled_from(EFFORTS)))
	local first_pick = draw(g.integers({ min = 1, max = #EFFORTS }))
	local next_pick = draw(g.integers({ min = 1, max = #EFFORTS }))
	local prefix = draw(g.sampled_from({ 100, effort.STICKY_TOKENS + 5000 }))
	local first = run({ jev = sure(first_pick) })
	local started = first:start(starting({ effort = chose }))
	local ran = chose or EFFORTS[first_pick]
	t.equal(started, chose == nil and tau.start({ effort = ran }) or nil)
	first:run_end({ stop = "Stop", turns = 1, cost = 0, text = "Done." })
	local second = run({ id = "r2", state = first.state, jev = sure(next_pick) })
	local next = second:start(starting({ input = "and the lanes", prefix = prefix }))
	local picked = EFFORTS[next_pick]
	local long = prefix >= effort.STICKY_TOKENS
	local expected = long and ran or picked
	t.equal(next, tau.start({ effort = expected }))
	local choice = second.state.choices[#second.state.choices]
	t.equal(choice.runs_at, expected)
	if choice.kept_for_cache ~= nil then
		t.equal(long and ran ~= picked, true)
		t.equal(choice.kept_for_cache >= effort.STICKY_TOKENS, true)
	else
		t.equal(not long or ran == picked, true)
	end
end)
