-- tau-goal in a run: a run goes on until Jev says the goal holds; a goal
-- stops when out of continuations or budget; it outlives the run and
-- takes what the interface changes; no goal means no checks, and a
-- failed check stops the run; and the banner, the command and the notes
-- say all of it.
local tau = require("tau")
local t = tau.test
local g = t.gen
local goal = require("goal")

-- A Jev answering each check with the next of `answers`, the last for
-- every later one, and keeping each request in `asked`.
local function jev(answers)
	local asked = {}
	local fake = {
		ask = function(request)
			table.insert(asked, request)
			local p = answers[math.min(#asked, #answers)]
			return { met = { probability = p } }, 0.001
		end,
	}
	return fake, asked
end

local function run(spec)
	return t.run({
		id = spec.id or "r1",
		state = spec.state,
		jev = spec.jev,
		kind = spec.kind,
		live = spec.live,
	})
end

local function stopping(r, text, turn, cost)
	r:turn_end({ turn = turn, text = text, calls = {}, cost = cost or 0 })
	return r:before_stop({ text = text, turn = turn, reason = "stop" })
end

t.case("the run goes on until Jev says the goal holds", function()
	local fake, asked = jev({ 0.1, 0.6999, goal.MET_AT })
	local r = run({ jev = fake })
	local started = r:start({ input = "/goal --budget 5 the tests pass" })
	-- The model gets the condition and how goals work, not the command.
	t.equal(started, tau.start({ input = goal.set_input("the tests pass") }))
	local first = stopping(r, "done?", 1)
	t.equal(first.decision, "continue")
	t.equal(string.sub(first.message, 1, #goal.CONTINUATION_PREFIX), goal.CONTINUATION_PREFIX)
	t.equal(string.find(first.message, "Goal: the tests pass", 1, true) ~= nil, true)
	local second = stopping(r, "done now?", 2)
	t.equal(string.find(second.message, "Continuation 2 of 10", 1, true) ~= nil, true)
	r:after_tool_result({ id = "c1", name = "bash", args = { command = "cargo nextest run" }, ok = true, text = "14 tests run: 14 passed" })
	t.equal(stopping(r, "14 passed.", 4), nil)
	-- The last check saw the tests' output.
	local last = asked[#asked]
	t.equal(last.state.goal, "the tests pass")
	t.equal(last.state.final_answer, "14 passed.")
	t.equal(last.state.recent_tool_results[1].output, "14 tests run: 14 passed")
	local kept = r.state.goal
	t.equal(kept.status, "met")
	t.equal(kept.continuations, 2)
	t.equal(kept.budget, 5)
	local checks = {}
	for _, check in ipairs(kept.checks) do
		table.insert(checks, { check.n, check.met, check.continuation or false })
	end
	t.equal(checks, { { 1, false, 1 }, { 2, false, 2 }, { 3, true, false } })
	t.equal(kept.spent > 0, true, "Jev's checks count toward the goal")
	-- Each check's note; the continuations they say draw as nothing.
	local view = r:view()
	t.equal(view.notes.g2 ~= nil, true)
	t.equal(view.messages[first.message], tau.ui.stack({}))
	t.equal(view.status.text, "met at check 3")
end)

t.case("a goal stops when out of continuations or budget", function()
	local r = run({ jev = jev({ 0.1 }) })
	r:start({ input = "/goal --continuations 1 the tests pass" })
	t.equal(stopping(r, "tried", 1).decision, "continue")
	t.equal(stopping(r, "tried again", 2), nil)
	t.equal(r.state.goal.status, "stopped")
	t.equal(r.state.goal.exhausted, "continuations")
	t.equal(r.state.notes.g2.text, "stopped the goal: out of continuations, and it is not met")

	-- Out of budget: each turn costs $0.25 of $0.30.
	local b = run({ jev = jev({ 0.1 }) })
	b:start({ input = "/goal --budget 0.30 the tests pass" })
	t.equal(stopping(b, "tried", 1, 0.25).decision, "continue")
	t.equal(stopping(b, "tried again", 2, 0.25), nil)
	t.equal(b.state.goal.exhausted, "budget")
	t.equal(b.state.goal.spent >= 0.5, true)
end)

t.case("a goal outlives the run and takes what the interface changes", function()
	local fake, asked = jev({ 0.1, 0.1, 0.9 })
	local first = run({ jev = fake })
	first:start({ input = "/goal --continuations 1 it works" })
	stopping(first, "tried", 1)
	stopping(first, "tried again", 2)
	first:run_end({ stop = "Stop", turns = 2, cost = 0, text = "tried again" })
	t.equal(#asked, 2)
	t.equal(first.state.goal.status, "stopped")

	-- Stopped: Keep going adds continuations, and a finished chat goes on.
	t.equal(first:action("act", { what = "keep_going" }), tau.interface("send", { text = goal.KEEP_GOING }))
	t.equal(first.state.goal.status, "active")
	t.equal(first.state.goal.max_continuations, 11)
	-- Paused: the next message is not checked.
	first:action("act", { what = "pause" })
	local paused = run({ id = "r2", state = first.state, jev = fake })
	paused:start({ input = "look at it again" })
	t.equal(stopping(paused, "looked", 1), nil)
	t.equal(#asked, 2)

	-- Resumed: met.
	paused:action("act", { what = "keep_going" })
	local resumed = run({ id = "r3", state = paused.state, jev = fake })
	resumed:start({ input = "keep going" })
	t.equal(stopping(resumed, "done", 1), nil)
	t.equal(resumed.state.goal.status, "met")
	t.equal(#resumed.state.goal.checks, 3)

	-- Cleared from a message: no goal left, and the model is told.
	local cleared = run({ id = "r4", state = resumed.state, jev = fake })
	t.equal(cleared:start({ input = "/goal clear" }), tau.start({ input = goal.CLEARED }))
	t.equal(cleared.state.goal, nil)
end)

t.case("no goal means no checks, and a failed check stops", function()
	local fake, asked = jev({ 0.1 })
	local r = run({ jev = fake })
	t.equal(r:start({ input = "hi" }), nil)
	t.equal(stopping(r, "hello", 1), nil)
	t.equal(#asked, 0)

	local failing = run({ jev = { ask = function() error("Jev answered 503", 0) end } })
	failing:start({ input = "/goal it works" })
	t.equal(stopping(failing, "hello", 1), nil)
	t.equal(failing.state.goal.status, "active", "still set, just unchecked")
	t.equal(string.find(failing.state.goal.error, "503", 1, true) ~= nil, true)
	t.equal(failing:view().notes.g0 ~= nil, true)
end)

t.case("without Jev, or in a sub-agent, a goal is not set", function()
	local keyless = run({})
	t.equal(keyless:start({ input = "/goal it works" }), nil)
	t.equal(keyless.state.goal, nil)
	local sub = run({ jev = jev({ 0.9 }), kind = "sub_agent" })
	t.equal(sub:start({ input = "/goal it works" }), nil)
	t.equal(sub.state.goal, nil)
end)

t.case("a /goal message draws as the goal it set, typed or as the model reads it", function()
	local r = run({ jev = jev({ 0.9 }) })
	r:start({ input = "/goal --continuations 3 the tests pass" })
	local view = r:view()
	local typed = view.messages["/goal --continuations 3 the tests pass"]
	t.equal(typed, view.messages[goal.set_input("the tests pass")])
	t.equal(string.find(json.encode(typed), "up to 3 continuations", 1, true) ~= nil, true)
	t.equal(view.row.count, "0/3")
	t.equal(view.row.icon, "target")
end)

t.case("the command sets the goal of a chat going on, or starts one", function()
	local live = run({ jev = jev({ 0.9 }), live = true })
	live:start({ input = "work" })
	local steer = live:action("/goal", { text = "it ships", drafts = { ["goal-continuations"] = "4" } })
	t.equal(steer, tau.interface("steer", { text = goal.set_input("it ships") }))
	t.equal(live.state.goal.max_continuations, 4)
	-- Limits typed with the command win over the popover's.
	local done = run({ jev = jev({ 0.9 }) })
	local sent = done:action("/goal", { text = "--budget 1 it ships", drafts = { ["goal-budget"] = "9" } })
	t.equal(sent, tau.interface("send", { text = "/goal --continuations 10 --budget 1.00 it ships" }))
	t.equal(done.state.goal, nil, "the message sets it as it starts")
	-- A condition that would not read back plain is sent quoted.
	local quoted = done:action("/goal", { text = '"clear"' })
	t.equal(quoted, tau.interface("send", { text = '/goal --continuations 10 --budget 2.00 "clear"' }))
	-- Without a key, it says what goals need.
	local keyless = run({})
	t.equal(keyless:action("/goal", { text = "it ships" }).tau_interface.action, "alert")
	-- `/goal` alone stays in the composer, to be written.
	t.equal(done:action("/goal", { text = "" }), tau.interface("composer", { text = "/goal " }))
end)

-- The banner's buttons follow where the goal stands, and Edit puts the
-- goal back in the composer.
t.property("the banner's buttons follow the goal", function(draw)
	local state = goal.state({})
	goal.set(state, "it works", 2, 1)
	local status = draw(g.sampled_from({ "active", "paused", "met", "stopped" }))
	state.goal.status = status
	state.checks = true
	local r = run({ state = state, jev = jev({ 0.5 }) })
	local banner = json.encode(r:view().banner)
	local labels = {
		active = { "Pause", "Edit", "Clear" },
		paused = { "Resume", "Edit", "Clear" },
		met = { "New goal", "Clear" },
		stopped = { "Keep going · +10", "Edit", "Clear" },
	}
	for _, label in ipairs(labels[status]) do
		t.equal(string.find(banner, '"label":"' .. label .. '"', 1, true) ~= nil, true, label)
	end
	local edit = r:action("act", { what = "edit" })
	t.equal(edit, tau.interface("composer", { text = "/goal " .. (status == "met" and "" or "it works") }))
end)

-- What a run cost while its stop went unchecked (Jev failed, or the goal
-- was paused) still counts toward the goal's budget: each message's start
-- set the count to nothing, so the budget counted less than was spent.
t.case("a run whose stop went unchecked still counts toward the budget", function()
	local down = true
	local fake = {
		ask = function()
			if down then
				error("Jev is down", 0)
			end
			return { met = { probability = 0.1 } }, 0.25
		end,
	}
	local r = run({ jev = fake })
	r:start({ input = "/goal --budget 5 the tests pass" })
	stopping(r, "done?", 1, 0.5)
	t.equal(#r.state.goal.checks, 0, "not checked")
	down = false
	r:start({ input = goal.KEEP_GOING })
	stopping(r, "done now?", 1, 0.2)
	t.equal(r.state.goal.spent, 0.5 + 0.2 + 0.25)
end)

-- An answer of Jev's with no verdict is a check that failed, said in a
-- note, not a failure of the plugin's.
t.case("an answer with no verdict is a failed check", function()
	local r = run({ jev = { ask = function() return {}, 0 end } })
	r:start({ input = "/goal the tests pass" })
	t.equal(stopping(r, "done?", 1), nil)
	t.equal(r.state.goal.error ~= nil, true, "noted")
	t.equal(#r.state.goal.checks, 0)
end)
