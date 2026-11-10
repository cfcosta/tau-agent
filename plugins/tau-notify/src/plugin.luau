-- Desktop notices (ADR 0045, 0050): it fills `tau.notify`, showing each
-- of tau's notices with `ctx.desktop.notice`, whose click opens the
-- notice's chat; its setting switches them off.
local tau = require("tau")

return tau.plugin({
	fills = { "tau.notify" },
	settings = {
		schema = {
			type = "object",
			properties = {
				on = {
					type = "boolean",
					title = "Notify me while tau is in the background",
					description = "A notice when a chat asks you something, is ready to land, failed or left conflicts on main, or something else waits on you. Clicking it opens the chat.",
				},
			},
		},
		default = { on = true },
	},
	notify = function(notice, ctx)
		if not ctx.settings.on then
			return nil
		end
		ctx.desktop.notice({ title = notice.title, body = notice.body })
		return nil
	end,
})
