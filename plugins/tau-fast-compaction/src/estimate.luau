-- The token estimate both stages use, `estimateTokens` of
-- `tamaratran/jev-pruner` (`src/jev.ts`; see THIRD_PARTY_NOTICES.md):
-- a word of ASCII letters is one token per six letters, rounded up; a
-- digit half a token; any other character that is not a space nine
-- tenths of a token, per UTF-16 unit. Counted exactly, in tenths, and
-- rounded up once; and counted by patterns, never one character at a
-- time, as a state can hold a whole history.
local estimate = {}

-- Tenths per digit in `tokens`.
estimate.DIGIT_TENTHS = 5

-- Tenths per digit in `state_tokens`: half a token more.
estimate.STATE_DIGIT_TENTHS = 10

-- White space to JavaScript's `\s` beyond ASCII's, as UTF-8.
local WIDE = {}
for _, c in ipairs({ 0xA0, 0x1680, 0x2028, 0x2029, 0x202F, 0x205F, 0x3000, 0xFEFF }) do
	table.insert(WIDE, utf8.char(c))
end
for c = 0x2000, 0x200A do
	table.insert(WIDE, utf8.char(c))
end

local function count(text, pattern)
	local _, n = string.gsub(text, pattern, "")
	return n
end

-- The estimate of `text` in tenths of a token, with `digit` tenths per
-- digit.
function estimate.tenths(text, digit)
	local tenths, letters = 0, 0
	for word in string.gmatch(text, "%a+") do
		local length = #word
		letters += length
		tenths += 10 * (1 + (length - 1) // 6)
	end
	local digits = count(text, "%d")
	local spaces = count(text, "[\t\n\v\f\r ]")
	for _, space in ipairs(WIDE) do
		if string.find(text, space, 1, true) then
			spaces += count(text, space)
		end
	end
	-- UTF-16 units: one per character, two for one past the Basic
	-- Multilingual Plane, which UTF-8 writes in four bytes.
	local units = utf8.len(text) + count(text, "[\240-\244]")
	return tenths + digit * digits + 9 * (units - letters - digits - spaces)
end

local function div_ceil(a, b)
	return (a + b - 1) // b
end
estimate.div_ceil = div_ceil

-- The estimate of `text`, in tokens.
function estimate.tokens(text)
	return div_ceil(estimate.tenths(text, estimate.DIGIT_TENTHS), 10)
end

-- The estimate of a Jev state or question set, as its JSON text: half a
-- token more per digit, since the numbers in JSON tokenize worse than
-- prose.
function estimate.state_tokens(json_text)
	return div_ceil(estimate.tenths(json_text, estimate.STATE_DIGIT_TENTHS), 10)
end

-- `state_tokens` of a JSON text, in tenths.
function estimate.json_tenths(json_text)
	return estimate.tenths(json_text, estimate.STATE_DIGIT_TENTHS)
end

-- `text` cut to `limit` characters, the last one an ellipsis.
function estimate.truncate(text, limit)
	if utf8.len(text) <= limit then
		return text
	end
	local kept = math.max(limit - 1, 0)
	local cut = utf8.offset(text, kept + 1)
	return string.sub(text, 1, cut - 1) .. "…"
end

return estimate
