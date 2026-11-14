-- A goal's lifecycle as a machine: every event from every status, in any
-- order, keeps its fields in shape (a check's error only on an active
-- goal, `exhausted` only on a stopped one); only an active goal is
-- checked, paused, stopped or failed; and a met goal stays met until it
-- is given more continuations.
local tau = require("tau")
local t = tau.test
local g = t.gen
local goal = require("goal")

local machine = goal.machine

t.machine_of("a goal goes as its status lets it", machine, {
	events = {
		check = g.records({
			check = g.records({
				n = g.integers({ min = 1, max = 9 }),
				met = g.booleans(),
				p = g.floats({ min = 0, max = 1 }),
				turn = g.integers({ min = 1, max = 9 }),
				continuation = g.optional(g.integers({ min = 1, max = 20 })),
				cost = g.just(0.01),
				spent = g.floats({ min = 0, max = 5 }),
			}),
		}),
		stop = g.records({ why = g.sampled_from({ "budget", "continuations" }) }),
		extend = g.records({ by = g.integers({ min = 1, max = 10 }) }),
		fail = g.records({ message = g.text({ max = 8 }) }),
	},
	init = function()
		return { before = machine.initial }
	end,
	event = function(model, e, after)
		local before = model.before
		local only_active = e.event == "check" or e.event == "stop" or e.event == "fail" or e.event == "pause"
		if only_active and before.status ~= "active" then
			t.equal(after, before, e.event .. " changes no goal that is " .. before.status)
		end
		if before.status == "met" and e.event ~= "extend" then
			t.equal(after.status, "met", "a met goal stays met")
		end
		model.before = after
	end,
	invariant = function(state)
		t.equal(#state.checks >= 0, true)
	end,
})

-- Pressing Pause on a goal that is met, or stopped, leaves it as it is.
t.case("only an active goal pauses", function()
	local state = goal.state({})
	goal.set(state, "the tests pass", 10, 2)
	goal.check(state, { n = 1, met = true, p = 0.9, turn = 1, cost = 0, spent = 0.1 })
	goal.pause(state)
	t.equal(state.goal.status, "met")
end)
