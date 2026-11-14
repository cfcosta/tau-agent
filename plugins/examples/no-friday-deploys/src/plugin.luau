-- A rule: blocks deploy commands on the days the person set.
local tau = require("tau")
local ui = tau.ui

local DAYS = { "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday" }

return tau.plugin {
	-- `before_tool` reaches every call, so its first version waits for
	-- the person's Allow.
	settings = {
		-- What can be set, checked before anything is saved.
		schema = {
			type = "object",
			properties = {
				days = { type = "array", items = { type = "string", enum = DAYS } },
			},
		},
		default = { days = { "Friday" } },
		-- Its own settings page; without one, a form is drawn from the
		-- schema.
		view = function(settings)
			return ui.stack({
				ui.rich("Deploy commands are blocked on these days."),
				ui.choice("days", DAYS, { multi = true }),
			})
		end,
	},
	-- `env.now` is the run's clock: hooks have no other.
	before_tool = function(call, _, env)
		local command = call.args.cmd
		if call.name == "exec_command" and type(command) == "string" and command:find("deploy") then
			if tau.contains(env.settings.days, env.now.weekday) then
				return tau.block("No deploys on " .. env.now.weekday .. ".")
			end
		end
	end,
}
