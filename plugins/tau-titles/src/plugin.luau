-- Run titles (ADR 0045, 0050): it fills `tau.title` with one short
-- request to the plan's newest Luna at low effort, as Codex names its
-- threads, the answer held to one short `title`.
local tau = require("tau")
local titles = require("titles")

return tau.plugin({
	fills = { "tau.title" },
	-- A family: the plan's newest of it, else the run's own model.
	uses = { infer = true, model = "luna" },
	settings = {
		schema = {
			type = "object",
			properties = {
				on = {
					type = "boolean",
					title = "Title chats with a model",
					description = "One short request as a chat starts, to the plan's smallest model. Off, a chat keeps the first line of its prompt.",
				},
			},
		},
		default = { on = true },
	},
	title = function(run, ctx)
		if not ctx.settings.on then
			return nil
		end
		local answer = ctx.tools.infer({
			task = titles.request(run.task),
			context = json.null,
			schema = titles.SCHEMA,
			effort = "low",
		})
		if not answer.ok then
			error(answer.error, 0)
		end
		return titles.clean(answer.value.title)
	end,
})
