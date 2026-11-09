-- A context rewrite: once a run has made many tool calls, it keeps what
-- the person said and the last few messages, and says what it left out.
local tau = require("tau")

-- The calls a run keeps before older ones go.
local KEPT = 6

return tau.plugin {
	settings = {
		schema = { type = "object", properties = { after = { type = "integer", minimum = 1 } } },
		default = { after = 20 },
	},
	rewrite_context = function(view, ctx)
		local results = 0
		for _, message in ipairs(view.messages) do
			if message.role == "toolResult" then
				results += 1
			end
		end
		if results <= ctx.settings.after then
			return nil
		end
		local count = #view.messages
		local keep = {}
		for _, message in ipairs(view.messages) do
			if message.role == "user" or message.index > count - KEPT then
				table.insert(keep, message.index)
			end
		end
		ctx.state.rewrites = (ctx.state.rewrites or 0) + 1
		return tau.rewrite({
			keep = keep,
			insert = {
				{
					before = count - KEPT + 1,
					text = "Older tool calls were left out to save room.",
				},
			},
			drops_conversation = true,
		})
	end,
	view = function(state, ctx)
		if state.rewrites then
			return { status = tau.ui.status("trimmed", tostring(state.rewrites) .. " times") }
		end
		return {}
	end,
}
