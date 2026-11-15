-- `heiko.text`: text by its characters. Text is UTF-8, as everything a
-- plugin is given is.

return function(heiko)
local text = {}
heiko.text = text

-- Unicode's white space past ASCII's, by code point.
local WIDE = {
	[0x85] = true,
	[0xA0] = true,
	[0x1680] = true,
	[0x2028] = true,
	[0x2029] = true,
	[0x202F] = true,
	[0x205F] = true,
	[0x3000] = true,
}
for code = 0x2000, 0x200A do
	WIDE[code] = true
end

-- Whether the code point `code` is white space, as Unicode has it
-- (`White_Space`): what Rust's `char::is_whitespace` says.
function text.is_space(code)
	return code == 0x20 or (code >= 0x09 and code <= 0x0D) or WIDE[code] == true
end

local is_space = text.is_space

-- `s` without the white space it starts and ends with.
function text.trim(s)
	local first, last = nil, 0
	for at, code in utf8.codes(s) do
		if not is_space(code) then
			first = first or at
			last = utf8.offset(s, 2, at) - 1
		end
	end
	if first == nil then
		return ""
	end
	return string.sub(s, first, last)
end

-- `s` on one line: each run of white space one space, none at its ends.
function text.one_line(s)
	local words, from = {}, nil
	for at, code in utf8.codes(s) do
		if is_space(code) then
			if from ~= nil then
				table.insert(words, string.sub(s, from, at - 1))
				from = nil
			end
		elseif from == nil then
			from = at
		end
	end
	if from ~= nil then
		table.insert(words, string.sub(s, from))
	end
	return table.concat(words, " ")
end

-- The first `chars` characters of `s`, and whether it has more.
function text.prefix(s, chars)
	if chars < 1 then
		return "", s ~= ""
	end
	local after = utf8.offset(s, chars + 1)
	if after == nil or after > #s then
		return s, false
	end
	return string.sub(s, 1, after - 1), true
end

-- `s` in at most `chars` characters: whole when it fits, else cut with
-- an ellipsis as its last character.
function text.cut(s, chars)
	local _, more = text.prefix(s, chars)
	if not more then
		return s
	end
	if chars < 1 then
		return ""
	end
	return text.prefix(s, chars - 1) .. "…"
end

-- The characters of `s`, as code points, in order.
function text.codes(s)
	local out = {}
	for _, code in utf8.codes(s) do
		table.insert(out, code)
	end
	return out
end

end
