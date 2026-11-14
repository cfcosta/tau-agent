-- The sign-in as a machine (ADR 0054): every event from every state,
-- in any order, keeps its states in shape; a token there with no one
-- signed in is always checked, a failure or not; a code past its
-- deadline is given up; and a check comes only from a token or an
-- approval.
local tau = require("tau")
local t = tau.test
local g = t.gen
local flow = require("flow")

local machine = flow.machine

t.machine_of("a sign-in checks every token it is given", machine, {
	events = {
		started = g.records({
			at = g.integers({ min = 0, max = 100 }),
			code = g.just({ code = "ABCD", url = "github.com/login/device", expires = "15 minutes" }),
			device_code = g.just("dev"),
			expires_in = g.integers({ min = 1, max = 50 }),
			interval = g.integers({ min = 0, max = 10 }),
		}),
		start_failed = g.records({ error = g.text({ max = 4 }) }),
		tick = g.records({ at = g.integers({ min = 0, max = 200 }) }),
		pending = g.records({ at = g.integers({ min = 0, max = 200 }) }),
		slow_down = g.records({ at = g.integers({ min = 0, max = 200 }) }),
		approved = g.records({ expires_in = g.integers({ min = 1, max = 100 }) }),
		refused = g.records({ error = g.text({ max = 4 }) }),
		checked = g.records({ ok = g.booleans(), error = g.text({ max = 4 }) }),
	},
	init = function()
		return { before = machine.initial }
	end,
	event = function(model, e, state, effects)
		local before = model.before
		local checks = false
		for _, effect in ipairs(effects) do
			if (type(effect) == "table" and effect[1] or effect) == "check_token" then
				checks = true
			end
		end
		if e.event == "token" and (before.state == "idle" or before.state == "failed") then
			t.equal(checks, true, "a token is checked, from " .. before.state)
		end
		if checks then
			t.equal(e.event == "token" or e.event == "approved", true, "a check only for a token or an approval")
		end
		if e.event == "tick" and before.state == "waiting" and e.at >= before.deadline then
			t.equal(state.state, "failed", "a code past its deadline is given up")
		end
		model.before = state
	end,
})

-- The case the model was written against: a refused token, then another.
t.case("a token after a refused one is checked, as the machine has it", function()
	local state = machine:step(nil, { event = "token" })
	state = machine:step(state, { event = "checked", ok = false, error = "no" })
	t.equal(state, { state = "failed", error = "no" })
	local _, effects = machine:step(state, { event = "token" })
	t.equal(effects, { "check_token" })
end)
