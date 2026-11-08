-- The token estimate (docs/reference/fast-compaction.md, "Token
-- estimate"): jev-pruner's, counted exactly in tenths.
local tau = require("tau")
local t = tau.test
local g = t.gen
local estimate = require("estimate")

t.case("the estimate matches jev-pruner's", function()
	t.equal(estimate.tokens(""), 0)
	t.equal(estimate.tokens("hello world"), 2)
	t.equal(estimate.tokens("abcdefgh"), 2)
	t.equal(estimate.tokens("abcdefghijklm"), 3)
	t.equal(estimate.tokens("12345"), 3)
	t.equal(estimate.tokens("a.b"), 3)
	t.equal(estimate.tokens('{"k":1}'), 6)
	t.equal(estimate.tokens("  \n\t "), 0)
	t.equal(estimate.tokens("é"), 1)
	-- Two UTF-16 units: 1.8.
	t.equal(estimate.tokens("🦀"), 2)
	-- JavaScript's `\s` has U+FEFF and not U+0085.
	t.equal(estimate.tokens("\u{feff}"), 0)
	t.equal(estimate.tokens("\u{85}"), 1)
	-- Exact tenths: ten symbols are 9, where a float sum gives 10.
	t.equal(estimate.tokens(".........."), 9)
	t.equal(estimate.state_tokens("12345"), 5)
	t.equal(estimate.state_tokens('{"k":1}'), 7)
end)

local LETTER = {}
for c = string.byte("a"), string.byte("z") do
	LETTER[string.char(c)] = true
	LETTER[string.upper(string.char(c))] = true
end
local SPACE = { [" "] = true, ["\n"] = true, ["\t"] = true, ["\u{a0}"] = true, ["\u{feff}"] = true }

-- jev-pruner's estimate as its regular expression reads the text, one
-- piece at a time: a run of ASCII letters, a digit, or one other
-- character, two for one of two UTF-16 units.
local function reference(pieces, digit)
	local tenths, run = 0, 0
	local function close()
		if run > 0 then
			tenths += 10 * (1 + (run - 1) // 6)
			run = 0
		end
	end
	for _, piece in ipairs(pieces) do
		if LETTER[piece] then
			run += 1
		else
			close()
			if string.match(piece, "^%d$") then
				tenths += digit
			elseif not SPACE[piece] then
				tenths += piece == "🦀" and 18 or 9
			end
		end
	end
	close()
	return (tenths + 9) // 10
end

t.property("both estimates match an independent reading", function(draw)
	local pieces = draw(g.lists(
		g.sampled_from({ "a", "b", "g", "X", "Z", "0", "1", "8", "9", ".", ",", "-", "{", "}", '"', " ", "\n", "\t", "\u{a0}", "\u{85}", "\u{feff}", "é", "日", "🦀" }),
		{ max = 40 }
	))
	local text = table.concat(pieces)
	t.equal(estimate.tokens(text), reference(pieces, 5))
	t.equal(estimate.state_tokens(text), reference(pieces, 10))
end, { cases = 300 })

t.case("a text is truncated to its characters, the last an ellipsis", function()
	t.equal(estimate.truncate("abc", 3), "abc")
	t.equal(estimate.truncate("abcd", 3), "ab…")
	t.equal(estimate.truncate("ééé", 2), "é…")
end)
