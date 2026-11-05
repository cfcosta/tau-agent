-- A slot filler: writes each run's title from its first sentence, with
-- no model.
local tau = require("tau")

return tau.plugin {
	name = "short-titles",
	description = "Titles runs by their first sentence.",
	fills = { "tau.title" },
	title = function(run, ctx)
		local task = string.match(run.task, "^%s*(.-)%s*$")
		local first = string.match(task, "^[^%.!?\n]+")
		if first == nil then
			return nil
		end
		first = string.gsub(first, "^%l", string.upper)
		if #first > 36 then
			first = string.sub(first, 1, 35) .. "…"
		end
		return first
	end,
}
