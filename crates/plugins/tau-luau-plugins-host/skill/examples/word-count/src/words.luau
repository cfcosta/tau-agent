-- Counting words: a module of the plugin's own, under `lib/`.
local words = {}

function words.count(text)
	local n = 0
	for _ in string.gmatch(text, "%S+") do
		n = n + 1
	end
	return n
end

return words
