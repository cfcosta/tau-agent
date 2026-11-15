-- `heiko.text`: white space as Unicode has it, and text cut by its
-- characters, not its bytes.
local heiko = require("heiko")
local t = heiko.test
local g = t.gen
local text = heiko.text

-- Characters of one to four bytes, and white space of both kinds.
local units = g.sampled_from({ "a", "Z", "é", "語", "🦀", " ", "\t", "\n", "\u{A0}", "\u{2003}", "\u{3000}" })

local function drawn(draw, max)
	return table.concat(draw(g.lists(units, { min = 0, max = max or 40 })))
end

local function space_at(s, byte)
	return text.is_space(utf8.codepoint(s, byte))
end

t.case("white space is Unicode's, and nothing else", function()
	for _, code in ipairs({ 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x20, 0x85, 0xA0, 0x1680, 0x2000, 0x200A, 0x2028, 0x2029, 0x202F, 0x205F, 0x3000 }) do
		t.equal(text.is_space(code), true, string.format("U+%04X", code))
	end
	-- The neighbours, and the zero-width ones, which are not white space.
	for _, code in ipairs({ 0x08, 0x0E, 0x1F, 0x21, 0x84, 0x86, 0x9F, 0xA1, 0x180E, 0x1FFF, 0x200B, 0x200D, 0x2060, 0xFEFF, 0x3001 }) do
		t.equal(text.is_space(code), false, string.format("U+%04X", code))
	end
end)

t.case("trim takes white space off both ends, of any width", function()
	t.equal(text.trim("  a b \n"), "a b")
	t.equal(text.trim("\u{3000}語\u{A0}"), "語")
	t.equal(text.trim(" \t\n"), "")
	t.equal(text.trim(""), "")
	t.equal(text.trim("🦀"), "🦀")
end)

t.property("trimmed text has no white space at its ends, and trims to itself", function(draw)
	local s = drawn(draw)
	local trimmed = text.trim(s)
	t.equal(text.trim(trimmed), trimmed, "once is enough")
	if trimmed ~= "" then
		t.equal(space_at(trimmed, 1), false, "its start")
		t.equal(space_at(trimmed, utf8.offset(trimmed, -1)), false, "its end")
	end
	-- What it took off was white space, and it kept the rest in place.
	local from = string.find(s, trimmed, 1, true)
	t.equal(from ~= nil, true, "a part of the text")
	t.equal(text.trim(string.sub(s, 1, from - 1)), "", "before")
	t.equal(text.trim(string.sub(s, from + #trimmed)), "", "after")
end)

t.case("one line has one space for each run of white space", function()
	t.equal(text.one_line("  a\n\n b\t\u{3000}c  "), "a b c")
	t.equal(text.one_line("\n \t"), "")
	t.equal(text.one_line("語"), "語")
end)

t.property("one line keeps the words, in order, with single spaces", function(draw)
	local s = drawn(draw)
	local line = text.one_line(s)
	t.equal(text.trim(line), line, "no white space at its ends")
	local words = {}
	local at = 1
	for _, code in utf8.codes(line) do
		if text.is_space(code) then
			t.equal(code, 0x20, "only spaces")
			at += 1
		else
			words[at] = (words[at] or "") .. utf8.char(code)
		end
	end
	for n = 1, at do
		t.equal(words[n] ~= nil or line == "", true, "no two spaces together")
	end
	t.equal(text.one_line(line), line, "once is enough")
end)

t.case("a prefix counts characters, and says whether more is left", function()
	t.equal({ text.prefix("héllo", 2) }, { "hé", true })
	t.equal({ text.prefix("héllo", 5) }, { "héllo", false })
	t.equal({ text.prefix("héllo", 9) }, { "héllo", false })
	t.equal({ text.prefix("héllo", 0) }, { "", true })
	t.equal({ text.prefix("", 0) }, { "", false })
end)

t.case("a cut text ends with an ellipsis, in the room it was given", function()
	t.equal(text.cut("retry-after", 20), "retry-after")
	t.equal(text.cut("retry-after", 11), "retry-after")
	t.equal(text.cut("retry-after", 6), "retry…")
	t.equal(text.cut("語語語語", 3), "語語…")
	t.equal(text.cut("ab", 1), "…")
	t.equal(text.cut("ab", 0), "")
end)

t.property("a cut text fits, and is the text's start when it was cut", function(draw)
	local s = drawn(draw)
	local chars = draw(g.integers({ min = 0, max = 50 }))
	local cut = text.cut(s, chars)
	t.equal(utf8.len(cut) <= chars, true, "it fits")
	if utf8.len(s) <= chars then
		t.equal(cut, s, "whole when it fits")
	elseif chars > 0 then
		local kept = string.sub(cut, 1, #cut - #"…")
		t.equal(string.sub(cut, #kept + 1), "…", "an ellipsis last")
		t.equal(string.sub(s, 1, #kept), kept, "the text's start")
		t.equal(utf8.len(cut), chars, "all the room")
	end
	local prefix, more = text.prefix(s, chars)
	t.equal(more, utf8.len(s) > chars, "more is what is past it")
	t.equal(utf8.len(prefix), math.min(chars, utf8.len(s)), "as many as asked, or all")
end)

t.property("the code points of a text make it again", function(draw)
	local s = drawn(draw)
	local codes = text.codes(s)
	t.equal(#codes, utf8.len(s))
	t.equal(utf8.char(table.unpack(codes)), s)
end)
