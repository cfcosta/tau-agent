-- A context rewrite: once a run has made many tool calls, it keeps what
-- the person said and the last few messages, and says what it left out.
local heiko = require("heiko")

-- The calls a run keeps before older ones go.
local KEPT = 6

return heiko.plugin {
	settings = {
		schema = { type = "object", properties = { after = { type = "integer", minimum = 1 } } },
		default = { after = 20 },
	},
	-- Every hook is `(input, model, env)`: `model` is the run's own model,
	-- changed in place, and `env` carries what the run gives it.
	rewrite_context = function(view, model, env)
		local results = 0
		for _, message in ipairs(view.messages) do
			if message.role == "toolResult" then
				results += 1
			end
		end
		if results <= env.settings.after then
			return nil
		end
		local count = #view.messages
		local keep = {}
		for _, message in ipairs(view.messages) do
			if message.role == "user" or message.index > count - KEPT then
				table.insert(keep, message.index)
			end
		end
		model.rewrites = (model.rewrites or 0) + 1
		return heiko.rewrite({
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
	-- `view` draws a frozen copy of the model.
	view = function(model)
		if model.rewrites then
			return { status = heiko.ui.status("trimmed", tostring(model.rewrites) .. " times") }
		end
		return {}
	end,
}
