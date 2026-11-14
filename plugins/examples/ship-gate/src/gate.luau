-- Whether a change may ship, as a machine (`tau.machine`): nothing asked,
-- a change waiting for the person, or the person's answer on it. A state
-- is a plain table the plugin keeps in its state, so it is saved and
-- shown on the phone as it is.
local tau = require("tau")

local gate = {}

local function asked(_, e)
	return { state = "waiting", change = e.change }
end

gate.machine = tau.machine({
	-- Every state, and the fields it carries: no others.
	states = {
		idle = {},
		waiting = { "change" },
		approved = { "change" },
		refused = { "change", "why?" },
	},
	initial = { state = "idle" },
	-- By event, then by the state it leaves. An event a state has no arm
	-- for changes nothing: a click on a stale button, a second answer.
	on = {
		ask = { idle = asked, waiting = asked, approved = asked, refused = asked },
		approve = {
			waiting = function(s)
				return { state = "approved", change = s.change }
			end,
		},
		refuse = {
			waiting = function(s, e)
				return { state = "refused", change = s.change, why = e.why }
			end,
		},
		-- The change shipped: the next one asks again.
		shipped = {
			approved = function()
				return { state = "idle" }
			end,
		},
	},
})

-- Whether `change` may ship now.
function gate.allows(state, change)
	return gate.machine:is(state, "approved") and state.change == change
end

return gate
