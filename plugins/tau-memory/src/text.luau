-- Text as the Rust tau-memory read it: the words it searches by
-- (`char::is_alphanumeric` runs and `_`, lowercased as `to_lowercase`
-- does, final sigma too), white space as `trim` reads it, and lines as
-- `str::lines` gives them. The tables are Rust's (`unicode`).
local unicode = require("unicode")

local M = {}

local ALNUM, SPACE = unicode.ALNUM, unicode.SPACE
local CASED, IGNORABLE = unicode.CASED, unicode.IGNORABLE
local has = unicode.has

local SIGMA = 0x3A3
local UNDERSCORE = 95

-- Whether `code` is part of a word: a letter, a digit or `_`.
local function in_word(code)
	if code < 128 then
		return (code >= 48 and code <= 57) or (code >= 65 and code <= 90) or (code >= 97 and code <= 122) or code == UNDERSCORE
	end
	return has(ALNUM, code)
end
M.in_word = in_word

-- Whether `code` is white space.
local function is_space(code)
	if code < 128 then
		return code == 32 or (code >= 9 and code <= 13)
	end
	return has(SPACE, code)
end
M.is_space = is_space

-- Whether the first character of `codes`, from `from` by `step`, that
-- is not case-ignorable is cased.
local function cased_after(codes, from, step)
	local at = from
	while at >= 1 and at <= #codes do
		local code = codes[at]
		if not has(IGNORABLE, code) then
			return has(CASED, code)
		end
		at += step
	end
	return false
end

-- The word made of `codes`, lowercased as `str::to_lowercase` does.
local function lower_word(codes)
	local out = table.create(#codes)
	for at, code in ipairs(codes) do
		if code < 128 then
			if code >= 65 and code <= 90 then
				code += 32
			end
			out[at] = string.char(code)
		elseif code == SIGMA then
			local final = cased_after(codes, at - 1, -1) and not cased_after(codes, at + 1, 1)
			out[at] = final and "ς" or "σ"
		else
			out[at] = unicode.lower(code)
		end
	end
	return table.concat(out)
end

-- The words of `text`, in order, lowercased: runs of letters, digits
-- and `_`. `retry_after` is one word, `retry-after` two.
function M.words(text)
	local found = {}
	local codes = {}
	for _, code in utf8.codes(text) do
		if in_word(code) then
			table.insert(codes, code)
		elseif #codes > 0 then
			table.insert(found, lower_word(codes))
			codes = {}
		end
	end
	if #codes > 0 then
		table.insert(found, lower_word(codes))
	end
	return found
end

-- The code points of `text`.
function M.codes(text)
	local codes = {}
	for _, code in utf8.codes(text) do
		table.insert(codes, code)
	end
	return codes
end

-- `text` without white space at either end.
function M.trim(text)
	local first, last
	local positions = {}
	for at, code in utf8.codes(text) do
		if not is_space(code) then
			first = first or at
			last = at
		end
		table.insert(positions, at)
	end
	if first == nil then
		return ""
	end
	-- `last` starts the last character kept: its end is the next start.
	local after = #text + 1
	for i, at in ipairs(positions) do
		if at == last then
			after = positions[i + 1] or (#text + 1)
			break
		end
	end
	return string.sub(text, first, after - 1)
end

-- `text` without white space at its end.
function M.trim_end(text)
	local after = 1
	for at, code in utf8.codes(text) do
		if not is_space(code) then
			after = at + #utf8.char(code)
		end
	end
	return string.sub(text, 1, after - 1)
end

-- Whether `text` is only white space, or empty.
function M.blank(text)
	for _, code in utf8.codes(text) do
		if not is_space(code) then
			return false
		end
	end
	return true
end

-- The lines of `text`, as `str::lines` gives them: split at `\n`, a
-- `\r` before it dropped, and no empty line after a last `\n`; a `\r`
-- at the very end stays.
function M.lines(text)
	local lines = {}
	local at = 1
	while at <= #text do
		local stop = string.find(text, "\n", at, true)
		local line
		if stop == nil then
			line = string.sub(text, at)
			at = #text + 1
		else
			line = string.sub(text, at, stop - 1)
			at = stop + 1
			-- Only a `\r` that ends the line with its `\n` goes.
			if string.sub(line, -1) == "\r" then
				line = string.sub(line, 1, -2)
			end
		end
		table.insert(lines, line)
	end
	return lines
end

-- How many characters `text` holds.
function M.length(text)
	return utf8.len(text) or #text
end

-- The first `n` characters of `text`, and whether there were more.
function M.prefix(text, n)
	local count = 0
	for at in utf8.codes(text) do
		if count == n then
			return string.sub(text, 1, at - 1), true
		end
		count += 1
	end
	return text, false
end

return M
