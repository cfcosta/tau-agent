-- The goals the demo shows, as the plugin leaves and draws them.
local tau = require("tau")
local t = tau.test

-- What Jev charges per million bytes of a request, as tau's fake Jev
-- counts them: one token a byte.
local PRICE_PER_MILLION_INPUT = 0.042

-- The request as Jev gets it: each yes/no question as
-- `{ type, instructions, criteria = { true, false } }`.
local function wire(request)
	local questions = {}
	for id, q in pairs(request.questions) do
		questions[id] = {
			type = q.kind,
			instructions = q.question,
			criteria = { ["true"] = q.yes, ["false"] = q.no },
		}
	end
	return { state = request.state, questions = questions }
end

-- A Jev that answers each check with the next of `answers`, charging
-- for the bytes of the request it gets.
local function jev(answers)
	local n = 0
	return {
		ask = function(request)
			n += 1
			local p = answers[n]
			if p == nil then
				error("no answer left", 0)
			end
			local cost = #json.encode(wire(request)) * PRICE_PER_MILLION_INPUT / 1000000
			return { met = { probability = p } }, cost
		end,
	}
end

-- A run on `command` that stops after each of `turns` with Jev's
-- answers, then ends: the state it leaves, and the view drawn from it.
local function goal_run(command, answers, turns)
	local r = t.run({ id = "demo", jev = jev(answers) })
	r:start({ input = command })
	for _, turn in ipairs(turns) do
		r:turn_end({ turn = turn, text = "", calls = array({}), cost = 0.12 })
		r:before_stop({ text = "done", turn = turn, reason = "stop" })
	end
	r:run_end({ stop = "Stop", turns = 0, cost = 0, text = "done" })
	return { state = r.state, view = r:view() }
end

local GOAL = "All tests in tau-ai pass and cargo clippy is clean"

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
