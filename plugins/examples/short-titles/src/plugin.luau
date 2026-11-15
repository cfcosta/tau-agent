-- A slot filler: writes each run's title from its first sentence, with
-- no model.
local heiko = require("heiko")

return heiko.plugin {
	fills = { "heiko.title" },
	title = function(run)
		local task = heiko.text.trim(run.task)
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
