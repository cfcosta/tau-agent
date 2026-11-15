-- The goals the demo shows, as the plugin leaves and draws them.
local heiko = require("heiko")
local t = heiko.test

-- A decider that answers each check with the next of `answers`.
local function decide(answers)
	local n = 0
	return {
		ask = function()
			n += 1
			local p = answers[n]
			if p == nil then
				error("no answer left", 0)
			end
			return { met = { probability = p } }, 0
		end,
	}
end

-- A run on `command` that stops after each of `turns` with the decider's
-- answers, then ends: the state it leaves, and the view drawn from it.
local function goal_run(command, answers, turns)
	local r = t.run({ id = "demo", decide = decide(answers) })
	r:start({ input = command })
	for _, turn in ipairs(turns) do
		r:turn_end({ turn = turn, text = "", calls = array({}), cost = 0, tokens = 120000 })
		r:before_stop({ text = "done", turn = turn, reason = "stop" })
	end
	r:run_end({ stop = "Stop", turns = 0, cost = 0, text = "done" })
	return { state = r.state, view = r:view() }
end

local GOAL = "All tests in heiko-ai pass and cargo clippy is clean"

t.fixture("working", function()
	return goal_run("/goal " .. GOAL, { 0.03, 0.08 }, { 8, 10 })
end)

t.fixture("met", function()
	return goal_run("/goal " .. GOAL, { 0.03, 0.08, 0.96 }, { 8, 10, 11 })
end)

t.fixture("stopped", function()
	return goal_run("/goal --continuations 2 " .. GOAL, { 0.12, 0.11, 0.11 }, { 9, 11, 13 })
end)

t.fixture("triage", function()
	return goal_run("/goal Every mutant in retry.rs is caught", { 0.2, 0.96 }, { 6, 9 })
end)

t.fixture("lanes", function()
	return goal_run("/goal --continuations 2 Every lane has an owner in lanes.toml", { 0.12, 0.11, 0.11 }, { 9, 11, 13 })
end)
