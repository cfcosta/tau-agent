-- A rule: blocks deploy commands on the days the person set.
local tau = require("tau")

return tau.plugin {
	name = "no-friday-deploys",
	description = "Blocks deploy commands on Fridays.",
	-- `before_tool` reaches every call, so its first version waits for
	-- the person's Allow.
	settings = {
		default = { days = { "Friday" } },
	},
	before_tool = function(call, ctx)
		local command = call.args.command
		if call.name == "bash" and type(command) == "string" and command:find("deploy") then
			if tau.contains(ctx.settings.days, ctx.now.weekday) then
				return tau.block("No deploys on " .. ctx.now.weekday .. ".")
			end
		end
	end,
}
