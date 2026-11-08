-- What tau-ask draws: the panel in the composer's place while a call
-- waits for the person (a tab per question and one to review, each
-- question's choices, a field for the person's own answer, a note per
-- answer, and the keys that work them), and the card of an `ask` call.
--
-- The panel takes the keys when it first appears. With them: up and
-- down move, a digit picks a row, Enter picks (and goes on, where one
-- answer is picked), Space toggles, left and right change questions,
-- and `n` opens the note of the question in view. In a field, Esc goes
-- back to the panel; Enter leaves the note, or takes the person's own
-- answer.
local tau = require("tau")
local ui = tau.ui
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

local function button(label, kind, action, args)
	return ui.button(label, action, args or {}, kind)
end

-- A tab per question, marked once answered, and one to review; what the
-- question in view takes, beside them.
local function tabs(questions, draft)
	local row = {}
	for i, question in ipairs(questions.questions) do
		local label = question.header
		if ask.answered(draft, i - 1) then
			label = "✓ " .. label
		end
		if ask.trim(draft.notes[i]) ~= "" then
			label ..= " ✎"
		end
		local kind = draft.tab == i - 1 and "primary" or "secondary"
		table.insert(row, button(label, kind, "go", { to = i - 1 }))
	end
	local last = #questions.questions
	local reviewing = ask.reviewing(questions, draft)
	table.insert(row, ui.mono("›"))
	table.insert(row, button("Submit", reviewing and "primary" or "secondary", "go", { to = last }))
	local mode
	if reviewing then
		mode = ask.count(draft) .. " of " .. last .. " answered"
	elseif questions.questions[draft.tab + 1].multi_select then
		mode = "pick any · " .. #draft.picked[draft.tab + 1] .. " chosen"
	else
		mode = "pick one"
	end
	table.insert(row, ui.spacer())
	table.insert(row, ui.toned(mode, "neutral", true))
	return ui.row(row)
end

-- A row's key, `›` before the row in focus; rows past the ninth take
-- none.
local function digit(k, focused)
	local key = k < 9 and tostring(k + 1) or " "
	return ui.mono((focused and "› " or "  ") .. key)
end

-- Row `k` (from 0) of question `i`: its key, whether it is picked, and
-- its label with what it means.
local function choice_row(draft, i, k, label, description)
	local picked = ask.has(draft.picked[i + 1], k)
	local cells = {
		digit(k, draft.cursor[i + 1] == k),
		button(label, picked and "primary" or "secondary", "choose", { row = k }),
	}
	if description ~= nil then
		table.insert(cells, ui.toned(description, "neutral"))
	end
	return ui.row(cells)
end

-- The question in view: its rows, the person's own answer, the preview
-- of the row in focus when its choices have previews, and its note.
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
	local pieces = { ui.rich("**" .. question.question .. "**") }
	for k, choice in ipairs(question.options) do
		table.insert(pieces, choice_row(draft, i, k - 1, choice.label, (not previews) and choice.description or nil))
	end
	local other = #question.options
	local own = {
		digit(other, draft.cursor[i + 1] == other),
		ui.input(panel.other_input(key.run, key.call, i), {
			placeholder = "Your own answer…",
			submit = "other-done",
			keep = true,
			focus = focus.other > 0 and focus.other or nil,
		}),
	}
	if draft.other_on[i + 1] then
		table.insert(own, ui.icon("check", "good"))
	end
	table.insert(pieces, ui.row(own))
	if previews then
		local focused = question.options[draft.cursor[i + 1] + 1]
		local side = { ui.toned(focused and focused.description or "Write your own answer.", "neutral") }
		if focused and focused.preview ~= nil then
			table.insert(side, ui.code(nil, focused.preview))
		end
		table.insert(pieces, ui.card(side))
	end
	if draft.note_open then
		table.insert(
			pieces,
			ui.input(panel.note_input(key.run, key.call, i), {
				placeholder = "Anything the agent should know about this answer…",
				multiline = true,
				submit = "note-done",
				keep = true,
				focus = focus.note > 0 and focus.note or nil,
			})
		)
	elseif ask.trim(draft.notes[i + 1]) ~= "" then
		table.insert(pieces, ui.row({ ui.icon("pencil", "info"), ui.toned(draft.notes[i + 1], "neutral") }))
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
			button("Edit", "secondary", "go", { to = i - 1 }),
		})
	end
	return ui.table({}, rows, { widths = { 1, 4, 1 } })
end

-- The foot: the keys, then what the panel can do from here.
local function foot(questions, draft)
	local reviewing = ask.reviewing(questions, draft)
	local hints
	if reviewing then
		hints = "← back · Enter send"
	elseif questions.questions[draft.tab + 1].multi_select then
		hints = "↑↓ move · Space toggle · n note · ←→ questions"
	else
		hints = "↑↓ move · Enter choose · n note · ←→ questions"
	end
	local row = {
		ui.toned(hints, "neutral", true),
		ui.spacer(),
		button("Cancel run", "danger", "tau.cancel"),
		button("Decline to answer", "secondary", "decline"),
	}
	if reviewing then
		local left = #questions.questions - ask.count(draft)
		local label = left == 0 and "Send answers" or ("Send answers · " .. left .. " left")
		table.insert(row, button(label, ask.ready(draft) and "primary" or "secondary", "send"))
	else
		local noted = ask.trim(draft.notes[draft.tab + 1]) ~= ""
		table.insert(row, button(noted and "Edit note" or "Add note", "secondary", "note-open"))
		table.insert(row, button("Next", "primary", "next"))
	end
	return ui.row(row)
end

-- The panel of the call `key.call` of the run `key.run`: an area that
-- takes the keys, holding the tabs, the question in view or the review,
-- and the foot. One area for every run's panel, as one composer is: the
-- keys stay with the panel from one waiting run to the next.
function panel.tree(key, questions, draft, focus)
	local children = { tabs(questions, draft) }
	if ask.reviewing(questions, draft) then
		table.insert(children, review(questions, draft))
	else
		table.insert(children, question_view(key, questions, draft, focus))
	end
	table.insert(children, foot(questions, draft))
	return ui.keys("ask-panel", panel.KEYS, "key", { ui.card(children) }, {
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
			table.insert(said, ui.row({ ui.icon("pencil", "info"), ui.toned(answer.note, "neutral") }))
		end
		table.insert(rows, { ui.toned(question.header, "neutral"), ui.stack(said) })
	end
	local pieces = { ui.table({}, rows, { widths = { 1, 4 } }) }
	if waiting then
		table.insert(pieces, ui.toned("Answer in the panel below.", "info"))
	end
	if reply and reply.declined then
		table.insert(pieces, ui.toned("You declined to answer.", "neutral"))
	end
	return ui.stack(pieces)
end

return panel
