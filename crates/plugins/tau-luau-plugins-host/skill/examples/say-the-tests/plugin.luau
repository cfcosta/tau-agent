-- A stop check: a run that edited files says which tests it ran before
-- it stops.
local tau = require("tau")
local ui = tau.ui

local EDITS = { write = true, edit = true }

return tau.plugin {
	name = "say-the-tests",
	description = "Asks a run that changed files to say which tests it ran.",
	turn_end = function(turn, ctx)
		for _, call in ipairs(turn.calls) do
			if EDITS[call.name] and call.ok then
				ctx.state.edited = true
			end
		end
	end,
	before_stop = function(stop, ctx)
		if ctx.state.edited and not stop.text:lower():find("test") then
			ctx.log("asked for the tests on turn " .. tostring(stop.turn))
			return tau.continue("You changed files: say which tests you ran, and how they went.")
		end
	end,
	view = function(state, ctx)
		if state.edited then
			return { status = ui.status("watching", "files changed", "info") }
		end
		return {}
	end,
}
