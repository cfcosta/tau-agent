-- Desktop notices (ADR 0045, 0050): it fills `heiko.notify`, showing each
-- of heiko's notices with a notice command, whose click opens the
-- notice's chat; its setting switches them off.
local heiko = require("heiko")

return heiko.plugin({
	fills = { "heiko.notify" },
	settings = {
		schema = {
			type = "object",
			properties = {
				on = {
					type = "boolean",
					title = "Notify me while heiko is in the background",
					description = "A notice when a chat asks you something, is ready to land, failed or left conflicts on main, or something else waits on you. Clicking it opens the chat.",
				},
			},
		},
		default = { on = true },
	},
	notify = function(notice, _, env)
		if not env.settings.on then
			return nil
		end
		heiko.give(heiko.cmd.notice({ title = notice.title, body = notice.body }))
		return nil
	end,
})
