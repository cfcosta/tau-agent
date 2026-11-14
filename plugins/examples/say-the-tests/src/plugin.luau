-- A stop check: a run that edited files says which tests it ran before
-- it stops.
local tau = require("tau")
local ui = tau.ui

local EDITS = { write = true, edit = true }

return tau.plugin {
	turn_end = function(turn, model)
		for _, call in ipairs(turn.calls) do
			if EDITS[call.name] and call.ok then
				model.edited = true
			end
		end
	end,
	before_stop = function(stop, model)
		if model.edited and not stop.text:lower():find("test") then
			tau.log("asked for the tests on turn " .. tostring(stop.turn))
			return tau.continue("You changed files: say which tests you ran, and how they went.")
		end
	end,
	view = function(model)
		if model.edited then
			return { status = ui.status("watching", "files changed", "info") }
		end
		return {}
	end,
}
