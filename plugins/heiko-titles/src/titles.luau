-- What a title is asked with, and what is kept of the answer.
local heiko = require("heiko")

local titles = {}

-- The longest title, in characters (heiko.title's limit).
titles.MAX_CHARS = 36

-- The longest request, in bytes: the instructions and the prompt.
titles.PROMPT_MAX_BYTES = 960

-- The schema the answer holds to: one nonempty `title`.
titles.SCHEMA = {
	type = "object",
	properties = {
		title = { type = "string", minLength = 1, maxLength = titles.MAX_CHARS },
	},
	required = { "title" },
	additionalProperties = false,
}

local function text_of(list, from, to)
	local out = {}
	for at = from, to do
		table.insert(out, utf8.char(list[at]))
	end
	return table.concat(out)
end

-- `text` without the characters `drop` keeps at either end (`ends`
-- "both", "start" or "end").
local function strip(text, drop, ends)
	local list = heiko.text.codes(text)
	local from, to = 1, #list
	if ends ~= "end" then
		while from <= to and drop[list[from]] do
			from += 1
		end
	end
	if ends ~= "start" then
		while to >= from and drop[list[to]] do
			to -= 1
		end
	end
	return text_of(list, from, to)
end

local function instructions()
	return "Generate a concise, single-line task title of at most "
		.. titles.MAX_CHARS
		.. " characters and under five words where possible. "
		.. "Start with an imperative verb. Capitalize only the first word "
		.. "unless the user's language, proper nouns, acronyms, or code terms "
		.. "require otherwise. Preserve ticket references exactly. Write in "
		.. "the user's language. Do not use quotes, markdown, or trailing "
		.. "punctuation. Do not answer the request."
end

-- The request for the title of a run about `task`, at most
-- PROMPT_MAX_BYTES: the task is cut short, never inside a character.
function titles.request(task)
	local prefix = instructions() .. "\n\nUser prompt:\n"
	local room = math.max(titles.PROMPT_MAX_BYTES - #prefix, 0)
	local kept = {}
	local used = 0
	for _, code in utf8.codes(heiko.text.trim(task)) do
		local char = utf8.char(code)
		if used + #char > room then
			break
		end
		used += #char
		table.insert(kept, char)
	end
	return prefix .. table.concat(kept)
end

local QUOTES = {}
for _, code in ipairs({ 0x22, 0x27, 0x60, 0x201C, 0x201D, 0x2018, 0x2019 }) do
	QUOTES[code] = true
end
local CLOSERS = { [0x2E] = true, [0x3F] = true, [0x21] = true }

-- The title kept of an answer's: without quotes around it, runs of
-- white space, or a final `.`, `?` or `!`, and at most MAX_CHARS. nil
-- when it comes out empty.
function titles.clean(title)
	local quoted = strip(heiko.text.trim(title), QUOTES, "both")
	local joined = heiko.text.one_line(quoted)
	local cut = heiko.text.trim(strip(joined, CLOSERS, "end"))
	if cut == "" then
		return nil
	end
	return (heiko.text.prefix(cut, titles.MAX_CHARS))
end

return titles
