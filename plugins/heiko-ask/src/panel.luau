-- What heiko-ask draws: the panel in the composer's place while a call
-- waits for the person (its head, a word per question and one to review,
-- the question large over its choices as full-width ruled rows, a field
-- for the person's own answer, a note per answer, and the keys that work them),
-- and the card of an `ask` call.
--
-- The panel takes the keys when it first appears. With them: up and
-- down move, a digit picks a row, Enter picks (and goes on, where one
-- answer is picked), Space toggles, left and right change questions,
-- and `n` opens the note of the question in view. In a field, Esc goes
-- back to the panel; Enter leaves the note, or takes the person's own
-- answer.
local heiko = require("heiko")
local ui = heiko.ui
local ask = require("ask")

local panel = {}

-- The keys the panel takes while it has them.
panel.KEYS = { "up", "down", "left", "right", "enter", "space", "n", "1", "2", "3", "4", "5", "6", "7", "8", "9", "escape" }

-- The input of the person's own answer to question `i` (from 0), and of
-- its note: the run's and the call's, as two runs' calls may share an
-- id.
function panel.other_input(run, call, i)
	return "ask-other-" .. run .. "-" .. call .. "-" .. i
end

function panel.note_input(run, call, i)
	return "ask-note-" .. run .. "-" .. call .. "-" .. i
end

local function button(label, kind, action, args, size)
	return ui.button(label, action, args or {}, kind, nil, size)
end

-- The panel's head: whose question it is, which of how many is in
-- view and how many answers it has, and the keys that work it.
local function head(questions, draft)
	local reviewing = ask.reviewing(questions, draft)
	local last = #questions.questions
	local hints, meta
	if reviewing then
		hints = "← back · ↵ send"
		meta = "review · " .. ask.count(draft) .. " of " .. last .. " answered"
	elseif questions.questions[draft.tab + 1].multi_select then
		hints = "↑↓ move · space toggle · n note"
		meta = (draft.tab + 1) .. " of " .. last .. " · " .. #draft.picked[draft.tab + 1] .. " chosen"
	else
		hints = "↑↓ move · n note"
		meta = (draft.tab + 1) .. " of " .. last
	end
	return ui.header("heiko asks", {
		meta = meta,
		actions = { ui.toned(hints, "neutral", true) },
	})
end

-- What parts two tab words: a mark with room at both its sides, in
-- spaces a line does not drop.
local function between(mark)
	return ui.toned("\u{A0}\u{A0}" .. mark .. "\u{A0}\u{A0}", "neutral")
end

-- A tab's word where it is the one in view: bright, and not a link.
local function current(label)
	return ui.rich("**" .. string.gsub(label, "[*`]", "") .. "**")
end

-- A word per question, marked once answered, and one to review, a dot
-- with room around it between each two and the one in view bright.
local function tabs(questions, draft)
	local row = {}
	local last = #questions.questions
	local reviewing = ask.reviewing(questions, draft)
	for i, question in ipairs(questions.questions) do
		local label = question.header
		if ask.answered(draft, i - 1) then
			label = "✓ " .. label
		end
		if heiko.text.trim(draft.notes[i]) ~= "" then
			label ..= " ✎"
		end
		if i > 1 then
			table.insert(row, between("·"))
		end
		if not reviewing and draft.tab == i - 1 then
			table.insert(row, current(label))
		else
			table.insert(row, button(label, "secondary", "go", { to = i - 1 }))
		end
	end
	table.insert(row, between("›"))
	if reviewing then
		table.insert(row, current("submit"))
	else
		table.insert(row, button("submit", "secondary", "go", { to = last }))
	end
	return ui.row(row)
end

-- A row's key; rows past the ninth take none.
local function digit(k)
	return k < 9 and tostring(k + 1) or " "
end

-- The shares of a ruled row's width: its key, then its label.
local RULED = { widths = { 1, 22 } }

-- A row's key in a square, outlined where the row is in focus; a click
-- on it carries out `action`.
local function key_square(k, focused, action, args)
	return ui.square(digit(k), focused and "outlined" or "quiet", focused and "info" or nil, action, args)
end

-- Row `k` (from 0) of question `i`, as wide as the panel: its key, then
-- its label. It is the large block once picked, what it means inside
-- it; where one answer is picked and none is yet, the row in focus is.
-- Any other row is a ruled one: its key in a square that picks it, its
-- label in plain text, a hairline under it, and `description` under the
-- label when given.
local function choice_row(draft, i, k, choice, description, multi)
	local picked = ask.has(draft.picked[i + 1], k)
	local focused = draft.cursor[i + 1] == k
	local on = picked or (not multi and #draft.picked[i + 1] == 0 and not draft.other_on[i + 1] and focused)
	if on then
		local label = (focused and "› " or "") .. digit(k) .. "  " .. choice.label
		return ui.button(label, "choose", { row = k }, "primary", nil, "large", true, choice.description), true
	end
	local label = ui.text(choice.label)
	if description ~= nil then
		label = ui.stack({ label, ui.toned(description, "neutral") })
	end
	return ui.table({}, { { key_square(k, focused, "choose", { row = k }), label } }, RULED), false
end

-- The question in view: the question, large; its choices as rows the
-- panel's width, the person's own answer under them; the preview of the
-- row in focus when its choices have previews; and its note.
local function question_view(key, questions, draft, focus)
	local i = draft.tab
	local question = questions.questions[i + 1]
	local previews = false
	if not question.multi_select then
		for _, choice in ipairs(question.options) do
			if choice.preview ~= nil then
				previews = true
			end
		end
	end
	local pieces = { ui.title(question.question) }
	-- The rows drawn as picked: each says what it means itself. Of the
	-- others, the one in focus has it under it, unless the preview's
	-- side does.
	local chosen = {}
	for k, choice in ipairs(question.options) do
		local focused = draft.cursor[i + 1] == k - 1
		local row, on =
			choice_row(draft, i, k - 1, choice, (focused and not previews) and choice.description or nil, question.multi_select)
		table.insert(pieces, row)
		chosen[k] = on
	end
	local other = #question.options
	local field = {
		ui.input(panel.other_input(key.run, key.call, i), {
			placeholder = "Your own answer…",
			submit = "other-done",
			keep = true,
			focus = focus.other > 0 and focus.other or nil,
		}),
	}
	if draft.other_on[i + 1] then
		table.insert(field, ui.mark("met"))
	end
	table.insert(pieces, ui.table({}, { { key_square(other, draft.cursor[i + 1] == other), ui.row(field) } }, RULED))
	if previews then
		-- What the row in focus means, unless its row says so already.
		local at = draft.cursor[i + 1] + 1
		local focused = question.options[at]
		local side = {}
		if not focused then
			table.insert(side, ui.toned("Write your own answer.", "neutral"))
		elseif not chosen[at] and focused.description ~= nil then
			table.insert(side, ui.toned(focused.description, "neutral"))
		end
		if focused and focused.preview ~= nil then
			table.insert(side, ui.code(nil, focused.preview))
		end
		table.insert(pieces, ui.stack(side))
	end
	if draft.note_open then
		table.insert(
			pieces,
			ui.input(panel.note_input(key.run, key.call, i), {
				placeholder = "A note for the agent…",
				multiline = true,
				submit = "note-done",
				keep = true,
				focus = focus.note > 0 and focus.note or nil,
			})
		)
	elseif heiko.text.trim(draft.notes[i + 1]) ~= "" then
		table.insert(pieces, ui.row({ ui.toned("note", "info", true), ui.toned(draft.notes[i + 1], "neutral") }))
	end
	return ui.stack(pieces)
end

-- The review: each question's answer, with a way back to it.
local function review(questions, draft)
	local rows = {}
	for i, question in ipairs(questions.questions) do
		local answered = ask.answered(draft, i - 1)
		table.insert(rows, {
			ui.toned(question.header, "neutral"),
			answered and ui.text(ask.answer_text(ask.answer(questions, draft, i - 1)))
				or ui.toned("not answered", "warn"),
			button("edit", "secondary", "go", { to = i - 1 }),
		})
	end
	return ui.table({}, rows, { widths = { 1, 4, 1 } })
end

-- The foot: what the panel can do from here, the way on the primary.
local function foot(questions, draft)
	local row = {}
	local reviewing = ask.reviewing(questions, draft)
	if not reviewing then
		local noted = heiko.text.trim(draft.notes[draft.tab + 1]) ~= ""
		table.insert(row, button(noted and "edit note" or "add a note", "secondary", "note-open"))
	end
	table.insert(row, button("decline", "secondary", "decline"))
	table.insert(row, button("cancel run", "danger", "heiko.cancel"))
	table.insert(row, ui.spacer())
	if reviewing then
		local left = #questions.questions - ask.count(draft)
		local label = left == 0 and "Send answers" or ("Send answers · " .. left .. " left")
		table.insert(row, button(label, ask.ready(draft) and "primary" or "secondary", "send"))
	else
		table.insert(row, button("Next", "primary", "next", nil, "medium"))
	end
	return ui.row(row)
end

-- The panel of the call `key.call` of the run `key.run`: an area that
-- takes the keys, holding the title, the tabs, the question in view or
-- the review, and the foot. One area for every run's panel, as one
-- composer is: the keys stay with the panel from one waiting run to the
-- next.
function panel.tree(key, questions, draft, focus)
	local children = { head(questions, draft), tabs(questions, draft) }
	if ask.reviewing(questions, draft) then
		table.insert(children, review(questions, draft))
	else
		table.insert(children, question_view(key, questions, draft, focus))
	end
	table.insert(children, foot(questions, draft))
	return ui.keys("ask-panel", panel.KEYS, "key", { ui.stack(children) }, {
		through = { "escape" },
		focus = focus.panel,
	})
end

-- The card's body of a call that asked: each question's header and its
-- answer, with the note under it; while it waits, where to answer; once
-- declined, that it was.
function panel.card(questions, reply, waiting)
	local answers = reply and reply.answers or nil
	local rows = {}
	for i, question in ipairs(questions.questions) do
		local answer = answers and answers[i] or nil
		local said = {}
		if answer then
			table.insert(said, ui.rich("**" .. ask.answer_text(answer) .. "**"))
		else
			table.insert(said, ui.toned(question.question, "neutral"))
		end
		if answer and answer.note ~= nil then
			table.insert(said, ui.row({ ui.toned("note", "info", true), ui.toned(answer.note, "neutral") }))
		end
		table.insert(rows, { ui.toned(question.header, "neutral"), ui.stack(said) })
	end
	local pieces = { ui.table({}, rows, { widths = { 1, 4 } }) }
	if waiting then
		table.insert(pieces, ui.row({ ui.mark("asks"), ui.toned("waiting for you", "warn") }))
	end
	if reply and reply.declined then
		table.insert(pieces, ui.toned("declined", "neutral"))
	end
	return ui.stack(pieces)
end

return panel
