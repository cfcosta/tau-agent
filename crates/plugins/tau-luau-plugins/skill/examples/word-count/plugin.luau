-- A tool with a card: counts a file's words, through the run's `read`.
local tau = require("tau")
local words = require("words")
local ui = tau.ui

return tau.plugin {
	name = "word-count",
	description = "Counts the words in a file.",
	-- The tools its own tools call. A new one here waits for an Allow.
	uses = { tools = { "read" } },
	tools = {
		word_count = {
			description = "Counts the words in a file of the workspace.",
			parameters = {
				type = "object",
				properties = { path = { type = "string" } },
				required = { "path" },
			},
			call = function(args, ctx)
				local text = ctx.tools.read({ path = args.path })
				local n = words.count(text)
				ctx.state.counted = (ctx.state.counted or 0) + 1
				return { path = args.path, words = n }
			end,
			card = function(call, result, ctx)
				return ui.row({
					ui.mono(result.path),
					ui.badge(tostring(result.words) .. " words", result.words > 1000 and "warn" or "neutral"),
				})
			end,
		},
	},
	view = function(state, ctx)
		if state.counted then
			return { status = ui.status("counted", tostring(state.counted) .. " files") }
		end
		return {}
	end,
}
