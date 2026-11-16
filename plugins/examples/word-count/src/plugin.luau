-- A tool with a card: counts a file's words, through the run's `read`.
local heiko = require("heiko")
local words = require("words")
local ui = heiko.ui
local cmd = heiko.cmd

return heiko.plugin {
	-- The tools its own tools call. A new one here waits for an Allow.
	uses = { tools = { "read" } },
	tools = {
		word_count = {
			kind = "look",
			description = "Counts the words in a file of the workspace.",
			parameters = {
				type = "object",
				properties = { path = { type = "string" } },
				required = { "path" },
			},
			call = function(args, model)
				-- A host tool is a command: `heiko.await` gives its answer. The
				-- model may change while it waits, so the count is read after.
				local text = heiko.await(cmd.tool("read", { path = args.path }))
				local n = words.count(text)
				model.counted = (model.counted or 0) + 1
				return { path = args.path, words = n }
			end,
			-- A card draws `{ call, result }`, and may not await.
			card = function(input)
				local result = input.result
				return ui.row({
					ui.mono(result.path),
					ui.badge(tostring(result.words) .. " words", result.words > 1000 and "warn" or "neutral"),
				})
			end,
		},
	},
	view = function(model)
		if model.counted then
			return { status = ui.status("counted", tostring(model.counted) .. " files") }
		end
		return {}
	end,
}
