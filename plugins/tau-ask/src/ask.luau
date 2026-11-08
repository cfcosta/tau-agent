-- What tau-ask knows of questions (docs/reference/ask.md): the limits a
-- call's questions keep, why ones that break them are refused, whether
-- answers fit what was asked, what the model reads back, and a
-- person's answers as they write them, with what each key does.
--
-- Rows, tabs and cursors count from 0, as the panel's buttons and keys
-- send them.
local ask = {}

-- The most questions one call asks.
ask.MAX_QUESTIONS = 4
-- The fewest and most choices a question offers, besides the person's
-- own answer.
ask.MIN_CHOICES = 2
ask.MAX_CHOICES = 4
-- The longest a question's header is, in characters: it is a tab.
ask.MAX_HEADER = 12
-- What the person's own answer is called: no choice may take it.
ask.OTHER = "Other"

-- What the model reads when the person would not answer.
ask.DECLINED = "The person declined to answer. Go on with your best judgment, or ask in plain words if you cannot."

-- What a sub-agent's `ask` answers: it has no one to ask.
ask.NO_ONE_TO_ASK =
	"A sub-agent has no one to ask: decide yourself, and say in your answer what you assumed and why."

-- White space as Unicode has it, by code point: what Rust's `trim` takes
-- off.
local SPACE = {}
for _, c in ipairs({ 0x20, 0x85, 0xA0, 0x1680, 0x2028, 0x2029, 0x202F, 0x205F, 0x3000 }) do
	SPACE[c] = true
end
for c = 0x09, 0x0D do
	SPACE[c] = true
end
for c = 0x2000, 0x200A do
	SPACE[c] = true
end

-- `text` without white space at either end.
local function trim(text)
	local first, last = nil, 0
	for at, c in utf8.codes(text) do
		if not SPACE[c] then
			first = first or at
			last = utf8.offset(text, 2, at) - 1
		end
	end
	if first == nil then
		return ""
	end
	return string.sub(text, first, last)
end
ask.trim = trim

-- Whether code point `c` is one Rust's `{:?}` escapes as `\u{...}`.
local ESCAPED = require("escaped")
local function escaped(c)
	local low, high = 1, #ESCAPED // 2
	while low <= high do
		local mid = (low + high) // 2
		local first, last = ESCAPED[2 * mid - 1], ESCAPED[2 * mid]
		if c < first then
			high = mid - 1
		elseif c > last then
			low = mid + 1
		else
			return true
		end
	end
	return false
end

-- `text` as Rust's `{:?}` writes a string: quoted, its quotes,
-- backslashes and the characters it does not print escaped.
local function debug_string(text)
	local out = { '"' }
	for _, c in utf8.codes(text) do
		local piece
		if c == 0x22 then
			piece = '\\"'
		elseif c == 0x5C then
			piece = "\\\\"
		elseif c == 0x0A then
			piece = "\\n"
		elseif c == 0x0D then
			piece = "\\r"
		elseif c == 0x09 then
			piece = "\\t"
		elseif c == 0x00 then
			piece = "\\0"
		elseif escaped(c) then
			piece = string.format("\\u{%x}", c)
		else
			piece = utf8.char(c)
		end
		table.insert(out, piece)
	end
	table.insert(out, '"')
	return table.concat(out)
end
ask.debug_string = debug_string

local function count(list)
	return list and #list or 0
end

-- Whether `value` is there: JSON's null is not.
local function present(value)
	return value ~= nil and value ~= json.null
end

-- Why the questions cannot be asked, if they cannot: the model reads it
-- and asks again. `nil` when they can.
function ask.check(questions)
	local n = count(questions.questions)
	if n < 1 or n > ask.MAX_QUESTIONS then
		return "Ask 1 to " .. ask.MAX_QUESTIONS .. " questions at a time, not " .. n .. "."
	end
	for i, question in ipairs(questions.questions) do
		local at = "Question " .. i
		local text = trim(question.question)
		if text == "" then
			return at .. " has no text."
		end
		for j = 1, i - 1 do
			if trim(questions.questions[j].question) == text then
				return at .. " repeats an earlier question."
			end
		end
		local header = trim(question.header)
		if header == "" or utf8.len(header) > ask.MAX_HEADER then
			return at
				.. "'s header must be 1 to "
				.. ask.MAX_HEADER
				.. " characters, not "
				.. debug_string(question.header)
				.. "."
		end
		local choices = count(question.options)
		if choices < ask.MIN_CHOICES or choices > ask.MAX_CHOICES then
			return at
				.. " must offer "
				.. ask.MIN_CHOICES
				.. " to "
				.. ask.MAX_CHOICES
				.. " choices, not "
				.. choices
				.. "."
		end
		for k, choice in ipairs(question.options) do
			local label = trim(choice.label)
			if label == "" then
				return at .. "'s choice " .. k .. " has no label."
			end
			if string.lower(label) == string.lower(ask.OTHER) then
				return at
					.. ' offers "'
					.. ask.OTHER
					.. '": the person can always write their own answer, so leave it out.'
			end
			for j = 1, k - 1 do
				if trim(question.options[j].label) == label then
					return at .. ' offers "' .. label .. '" twice.'
				end
			end
			if question.multi_select and present(choice.preview) then
				return at .. " picks several choices, so its choices cannot have previews."
			end
		end
	end
	return nil
end

-- The questions the model sent, read as a call's `Ask` reads them: each
-- question's `question`, `header` and `options` (each `label` and
-- `description`, maybe `preview`), and `multi_select`, false when not
-- given. `nil` and why when they do not read.
function ask.read(args)
	if type(args) ~= "table" or type(args.questions) ~= "table" then
		return nil, "missing field `questions`"
	end
	local questions = array({})
	for i, question in ipairs(args.questions) do
		local at = "questions[" .. (i - 1) .. "]"
		if type(question) ~= "table" then
			return nil, at .. " is not a question"
		end
		for _, field in ipairs({ "question", "header" }) do
			if type(question[field]) ~= "string" then
				return nil, at .. ": `" .. field .. "` is not a string"
			end
		end
		if type(question.options) ~= "table" then
			return nil, at .. ": missing field `options`"
		end
		local multi = question.multi_select
		if multi ~= nil and type(multi) ~= "boolean" then
			return nil, at .. ": `multi_select` is not a boolean"
		end
		local options = array({})
		for k, choice in ipairs(question.options) do
			local where = at .. ".options[" .. (k - 1) .. "]"
			if type(choice) ~= "table" then
				return nil, where .. " is not a choice"
			end
			for _, field in ipairs({ "label", "description" }) do
				if type(choice[field]) ~= "string" then
					return nil, where .. ": `" .. field .. "` is not a string"
				end
			end
			local preview = present(choice.preview) and choice.preview or nil
			if preview ~= nil and type(preview) ~= "string" then
				return nil, where .. ": `preview` is not a string"
			end
			table.insert(options, {
				label = choice.label,
				description = choice.description,
				preview = preview,
			})
		end
		table.insert(questions, {
			question = question.question,
			header = question.header,
			options = options,
			multi_select = multi == true,
		})
	end
	return { questions = questions }
end

-- What the person wrote, trimmed; `nil` when it is blank.
local function own(answer)
	if not present(answer.other) then
		return nil
	end
	local text = trim(answer.other)
	if text == "" then
		return nil
	end
	return text
end

-- An answer in a line: the labels picked, then what the person wrote,
-- joined by commas.
function ask.answer_text(answer)
	local parts = {}
	for _, label in ipairs(answer.picked or {}) do
		table.insert(parts, label)
	end
	local written = own(answer)
	if written then
		table.insert(parts, written)
	end
	return table.concat(parts, ", ")
end

-- Why `reply` does not answer `questions`, if it does not: an answer per
-- question, each picking the question's own choices, one for a question
-- where one is picked, and none empty. `nil` when it does.
function ask.check_reply(questions, reply)
	if reply.declined then
		return nil
	end
	local answers = reply.answers or {}
	local asked = questions.questions
	if #answers ~= #asked then
		return #answers .. " answers for " .. #asked .. " questions."
	end
	for i, question in ipairs(asked) do
		local answer = answers[i]
		local picked = answer.picked or {}
		local given = #picked + (own(answer) and 1 or 0)
		if given == 0 then
			return '"' .. question.header .. '" has no answer.'
		end
		if not question.multi_select and given > 1 then
			return '"' .. question.header .. '" takes one answer, not ' .. given .. "."
		end
		local at = {}
		for _, label in ipairs(picked) do
			local found = nil
			for k, choice in ipairs(question.options) do
				if choice.label == label then
					found = k
					break
				end
			end
			if found == nil then
				return '"' .. question.header .. '" offers no choice "' .. label .. '".'
			end
			table.insert(at, found)
		end
		for k = 2, #at do
			if at[k - 1] >= at[k] then
				return '"' .. question.header .. '" picks its choices more than once or out of order.'
			end
		end
	end
	return nil
end

-- What the model reads back for `questions`, answered with `reply`.
function ask.reply_text(questions, reply)
	if reply.declined then
		return ask.DECLINED
	end
	local lines = { "The person answered:" }
	for i, question in ipairs(questions.questions) do
		local answer = (reply.answers or {})[i]
		if answer == nil then
			break
		end
		table.insert(lines, '\n"' .. trim(question.question) .. '" = "' .. ask.answer_text(answer) .. '"')
		local note = present(answer.note) and trim(answer.note) or ""
		if note ~= "" then
			table.insert(lines, "\n  note: " .. string.gsub(note, "\n", "\n        "))
		end
	end
	return table.concat(lines)
end

-- A person's answers to one call as they write them (ADR 0045: the
-- run's state): the question in view (`tab`; one past the last is the
-- review), each question's row in focus (a choice, or one past the last
-- for the person's own answer), the rows picked, what they wrote and
-- whether it counts, each note, and whether the note in view is open.
function ask.draft(questions)
	local draft = {
		tab = 0,
		cursor = array({}),
		picked = array({}),
		other = array({}),
		other_on = array({}),
		notes = array({}),
		note_open = false,
	}
	for _ in ipairs(questions.questions) do
		table.insert(draft.cursor, 0)
		table.insert(draft.picked, array({}))
		table.insert(draft.other, "")
		table.insert(draft.other_on, false)
		table.insert(draft.notes, "")
	end
	return draft
end

local function n_of(questions)
	return #questions.questions
end

-- Whether the review is in view.
function ask.reviewing(questions, draft)
	return draft.tab >= n_of(questions)
end

-- The row of the person's own answer in question `i` (from 0).
local function other_row(questions, i)
	return #questions.questions[i + 1].options
end

local function has(list, value)
	for _, item in ipairs(list) do
		if item == value then
			return true
		end
	end
	return false
end

-- Whether question `i` (from 0) has an answer.
function ask.answered(draft, i)
	return #draft.picked[i + 1] > 0 or (draft.other_on[i + 1] and trim(draft.other[i + 1]) ~= "")
end

-- How many questions have an answer.
function ask.count(draft)
	local n = 0
	for i = 0, #draft.picked - 1 do
		if ask.answered(draft, i) then
			n += 1
		end
	end
	return n
end

-- Whether every question has an answer.
function ask.ready(draft)
	return ask.count(draft) == #draft.picked
end

-- The answer to question `i` (from 0), as it stands: the labels picked,
-- in the question's order, what the person wrote, and the note.
function ask.answer(questions, draft, i)
	local question = questions.questions[i + 1]
	local rows = {}
	for _, row in ipairs(draft.picked[i + 1]) do
		table.insert(rows, row)
	end
	table.sort(rows)
	local picked = array({})
	for _, row in ipairs(rows) do
		table.insert(picked, question.options[row + 1].label)
	end
	local answer = { picked = picked }
	local written = trim(draft.other[i + 1])
	if draft.other_on[i + 1] and written ~= "" then
		answer.other = written
	end
	local note = trim(draft.notes[i + 1])
	if note ~= "" then
		answer.note = note
	end
	return answer
end

-- The reply, once every question has an answer; `nil` before.
function ask.reply(questions, draft)
	if not ask.ready(draft) then
		return nil
	end
	local answers = array({})
	for i = 0, n_of(questions) - 1 do
		table.insert(answers, ask.answer(questions, draft, i))
	end
	return { answers = answers }
end

-- Shows question `tab`, or the review one past the last.
function ask.go(questions, draft, tab)
	draft.tab = math.min(tab, n_of(questions))
	draft.note_open = false
end

-- Picks row `row` of the question in view: a choice, or the person's
-- own answer. A one-answer question goes on to the next when `advance`.
-- What the panel does next: "stay", or "write_other" to put the keys in
-- the field of the person's own answer.
function ask.choose(questions, draft, row, advance)
	local i = draft.tab
	if ask.reviewing(questions, draft) or row > other_row(questions, i) then
		return "stay"
	end
	draft.cursor[i + 1] = row
	local multi = questions.questions[i + 1].multi_select
	if row == other_row(questions, i) then
		if multi and draft.other_on[i + 1] then
			draft.other_on[i + 1] = false
			return "stay"
		end
		draft.other_on[i + 1] = true
		if not multi then
			draft.picked[i + 1] = array({})
		end
		return "write_other"
	end
	if multi then
		local picked = draft.picked[i + 1]
		local kept = array({})
		for _, k in ipairs(picked) do
			if k ~= row then
				table.insert(kept, k)
			end
		end
		if #kept == #picked then
			table.insert(kept, row)
			table.sort(kept)
		end
		draft.picked[i + 1] = kept
		return "stay"
	end
	draft.picked[i + 1] = array({ row })
	draft.other_on[i + 1] = false
	if advance then
		ask.go(questions, draft, i + 1)
	end
	return "stay"
end

-- Takes what the person wrote as their own answer to the question in
-- view: written, it counts, in place of a choice where one is picked.
function ask.write_other(questions, draft, text)
	local i = draft.tab
	if ask.reviewing(questions, draft) or draft.other[i + 1] == text then
		return
	end
	draft.other[i + 1] = text
	if trim(text) ~= "" then
		draft.other_on[i + 1] = true
		if not questions.questions[i + 1].multi_select then
			draft.picked[i + 1] = array({})
		end
	end
end

-- Takes the note of the question in view.
function ask.write_note(questions, draft, text)
	if not ask.reviewing(questions, draft) then
		draft.notes[draft.tab + 1] = text
	end
end

-- Enter in the field of the person's own answer: a one-answer question
-- goes on to the next.
function ask.other_done(questions, draft)
	local i = draft.tab
	if
		not ask.reviewing(questions, draft)
		and not questions.questions[i + 1].multi_select
		and ask.answered(draft, i)
	then
		ask.go(questions, draft, i + 1)
	end
end

-- The key a keystroke names, if the panel takes it: `"up"`, `"down"`,
-- `"left"`, `"right"`, `"enter"`, `"space"`, `"n"` (the note), or a
-- digit from 1 to 9 as a number.
function ask.key_of(name)
	if
		name == "up"
		or name == "down"
		or name == "left"
		or name == "right"
		or name == "enter"
		or name == "space"
		or name == "n"
	then
		return name
	end
	if type(name) == "string" and string.match(name, "^%+?%d+$") then
		local n = tonumber((string.gsub(name, "^%+", "")))
		if n ~= nil and n >= 1 and n <= 9 then
			return n
		end
	end
	return nil
end

-- What `key` does while the panel has the keys: "stay", "write_other",
-- "write_note" (the keys to the note's field), or "send".
function ask.key(questions, draft, key)
	if key == "left" then
		ask.go(questions, draft, math.max(draft.tab - 1, 0))
		return "stay"
	elseif key == "right" then
		ask.go(questions, draft, draft.tab + 1)
		return "stay"
	end
	if ask.reviewing(questions, draft) then
		if key == "enter" and ask.ready(draft) then
			return "send"
		end
		return "stay"
	end
	local i = draft.tab
	local multi = questions.questions[i + 1].multi_select
	local rows = other_row(questions, i) + 1
	if key == "up" then
		draft.cursor[i + 1] = math.max(draft.cursor[i + 1] - 1, 0)
	elseif key == "down" then
		draft.cursor[i + 1] = math.min(draft.cursor[i + 1] + 1, rows - 1)
	elseif type(key) == "number" then
		if key <= rows then
			return ask.choose(questions, draft, key - 1, not multi)
		end
	elseif key == "space" then
		return ask.choose(questions, draft, draft.cursor[i + 1], false)
	elseif key == "enter" then
		if multi then
			ask.go(questions, draft, i + 1)
		else
			return ask.choose(questions, draft, draft.cursor[i + 1], true)
		end
	elseif key == "n" then
		draft.note_open = true
		return "write_note"
	end
	return "stay"
end

ask.has = has

return ask
