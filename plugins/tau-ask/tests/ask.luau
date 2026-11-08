-- What holds for any questions and any keys: questions within the
-- limits are asked and any one fault is refused; however the person
-- works the panel, what it sends answers what was asked; the model
-- reads every answer and note.
local tau = require("tau")
local t = tau.test
local g = t.gen
local ask = require("ask")

-- Distinct words, so labels and questions never repeat by chance.
local function word(draw, n)
	return draw(g.from_regex("^[a-z]{1,6}$")) .. n
end

-- `n` characters a header may hold, of one to three bytes each, none of
-- them white space at an end.
local function header_of(draw, n)
	local ends = g.sampled_from({ "a", "é", "漢" })
	local first = draw(ends)
	if n == 1 then
		return first
	end
	local inner = draw(g.lists(g.sampled_from({ "a", "Z", "1", " ", "é", "漢" }), { min = n - 2, max = n - 2 }))
	return first .. table.concat(inner) .. draw(ends)
end

local function question(draw, n)
	local multi = draw(g.booleans())
	local choices = draw(g.integers({ min = 2, max = ask.MAX_CHOICES }))
	-- The shortest and longest headers as often as the rest.
	local length = draw(g.one_of({ g.sampled_from({ 1, ask.MAX_HEADER }), g.integers({ min = 1, max = ask.MAX_HEADER }) }))
	local options = array({})
	for k = 0, choices - 1 do
		local preview = nil
		if not multi and draw(g.booleans()) then
			preview = word(draw, k)
		end
		table.insert(options, { label = word(draw, k), description = word(draw, k), preview = preview })
	end
	return {
		question = word(draw, n) .. "?",
		header = header_of(draw, length),
		options = options,
		multi_select = multi,
	}
end

local questions = g.composite(function(draw)
	local n = draw(g.integers({ min = 1, max = ask.MAX_QUESTIONS }))
	local list = array({})
	for i = 0, n - 1 do
		table.insert(list, question(draw, i))
	end
	return { questions = list }
end)

local function copy(value)
	if type(value) ~= "table" then
		return value
	end
	local out = {}
	for key, item in pairs(value) do
		out[key] = copy(item)
	end
	return out
end

local function starts(text, prefix)
	return string.sub(text, 1, #prefix) == prefix
end

-- Questions within the limits are asked, a header of 1 to 12 characters
-- however many bytes they take; each kind of fault alone is refused,
-- with a reason that names the question at fault. Text is compared
-- trimmed: a question or a label repeated with white space around it is
-- still repeated.
t.property("questions within the limits are asked and faults refused", function(draw)
	local asked = draw(questions)
	t.equal(ask.check(asked), nil)
	local wrong = copy(asked)
	local n = #asked.questions
	local i = draw(g.integers({ min = 1, max = n }))
	local at = i
	local fault = draw(g.integers({ min = 0, max = 7 }))
	if fault == 0 then
		wrong.questions = {}
		at = nil
	elseif fault == 1 then
		local extra = asked.questions[1]
		while #wrong.questions <= ask.MAX_QUESTIONS do
			local more = copy(extra)
			more.question = more.question .. " " .. #wrong.questions
			table.insert(wrong.questions, more)
		end
		at = nil
	elseif fault == 2 then
		local length = draw(g.one_of({ g.just(ask.MAX_HEADER + 1), g.integers({ min = ask.MAX_HEADER + 1, max = ask.MAX_HEADER + 6 }) }))
		wrong.questions[i].header = header_of(draw, length)
	elseif fault == 3 then
		wrong.questions[i].options = { wrong.questions[i].options[1] }
	elseif fault == 4 then
		wrong.questions[i].options[2].label = string.lower(ask.OTHER)
	elseif fault == 5 then
		wrong.questions[i].options[2].label = " " .. wrong.questions[i].options[1].label .. " "
	elseif fault == 6 then
		wrong.questions[i].multi_select = true
		wrong.questions[i].options[1].preview = "x"
	else
		local again = " " .. wrong.questions[1].question .. "\t"
		if #wrong.questions < 2 then
			local twin = copy(wrong.questions[1])
			twin.question = again
			table.insert(wrong.questions, twin)
		else
			wrong.questions[2].question = again
		end
		at = 2
	end
	local why = ask.check(wrong)
	t.equal(why ~= nil, true, "one fault is refused")
	if at ~= nil then
		t.equal(starts(why, "Question " .. at), true, why)
	end
end, { cases = 300 })

-- What the person does in the panel.
local step = g.composite(function(draw)
	local kind = draw(g.integers({ min = 0, max = 12 }))
	if kind <= 6 then
		return { key = ({ "up", "down", "left", "right", "enter", "space", "n" })[kind + 1] }
	elseif kind == 7 then
		return { key = draw(g.integers({ min = 1, max = 9 })) }
	elseif kind == 8 then
		return { click = draw(g.integers({ min = 0, max = 6 })) }
	elseif kind == 9 then
		return { other = draw(g.sampled_from({ "", "  ", "my own", "two\nlines" })) }
	elseif kind == 10 then
		return { note = draw(g.sampled_from({ "", "a note", " spaced " })) }
	end
	return { go = draw(g.integers({ min = 0, max = 6 })) }
end)

-- However the person works the panel, it stays on a question or the
-- review, a one-answer question holds at most one answer, and what it
-- would send answers what was asked; Send comes only when it is ready.
t.property("whatever the person does, the panel sends a fitting reply", function(draw)
	local asked = draw(questions)
	local steps = draw(g.lists(step, { max = 40 }))
	local draft = ask.draft(asked)
	for _, s in ipairs(steps) do
		local follows = "stay"
		if s.key ~= nil then
			follows = ask.key(asked, draft, s.key)
		elseif s.click ~= nil then
			follows = ask.choose(asked, draft, s.click, false)
		elseif s.other ~= nil then
			ask.write_other(asked, draft, s.other)
		elseif s.note ~= nil then
			ask.write_note(asked, draft, s.note)
		else
			ask.go(asked, draft, s.go)
		end
		t.equal(draft.tab <= #asked.questions, true, "on a question or the review")
		for i, question in ipairs(asked.questions) do
			t.equal(draft.cursor[i] <= #question.options, true, "the cursor on a row")
			local answer = ask.answer(asked, draft, i - 1)
			local given = #answer.picked + (answer.other and 1 or 0)
			if not question.multi_select then
				t.equal(given <= 1, true, "one answer")
			end
			t.equal(ask.answered(draft, i - 1), given > 0)
		end
		if follows == "send" then
			t.equal(ask.reviewing(asked, draft) and ask.ready(draft), true, "sent when ready")
		end
		local reply = ask.reply(asked, draft)
		if reply ~= nil then
			t.equal(ask.ready(draft), true)
			t.equal(ask.check_reply(asked, reply), nil)
		else
			t.equal(ask.ready(draft), false)
		end
	end
end, { cases = 300 })

-- The model reads each question with its answer (choices and the
-- person's own, trimmed), and each note.
t.property("the model reads every answer and note", function(draw)
	local asked = draw(questions)
	local answers = array({})
	for _, question in ipairs(asked.questions) do
		-- A checklist picks any of its choices, in order; a question
		-- where one is picked takes a choice or the person's own.
		local picked = array({})
		for _, choice in ipairs(question.options) do
			if draw(g.booleans()) then
				table.insert(picked, choice.label)
			end
		end
		if not question.multi_select then
			picked = array({ picked[1] })
		end
		local own = #picked == 0 or draw(g.booleans())
		if own and not question.multi_select then
			picked = array({})
		end
		table.insert(answers, {
			picked = picked,
			other = own and draw(g.sampled_from({ "my own", "  padded  " })) or nil,
			note = draw(g.optional(g.sampled_from({ "first line\nsecond", " short " }))),
		})
	end
	local reply = { answers = answers }
	t.equal(ask.check_reply(asked, reply), nil)
	-- As ask.md writes it: each question with its picks in the
	-- question's order, then the person's own answer, trimmed, joined by
	-- commas; a note under it, its lines kept in line.
	local want = "The person answered:"
	for i, question in ipairs(asked.questions) do
		local answer = answers[i]
		local parts = {}
		for _, label in ipairs(answer.picked) do
			table.insert(parts, label)
		end
		if answer.other then
			table.insert(parts, ask.trim(answer.other))
		end
		want ..= '\n"' .. question.question .. '" = "' .. table.concat(parts, ", ") .. '"'
		if answer.note then
			local lines = string.split(ask.trim(answer.note), "\n")
			want ..= "\n  note: " .. table.concat(lines, "\n        ")
		end
	end
	t.equal(ask.reply_text(asked, reply), want)
	t.equal(starts(ask.reply_text(asked, { declined = true }), "The person declined to answer."), true)
end, { cases = 200 })

t.case("answers that do not fit what was asked say why", function()
	local asked = {
		questions = {
			{ question = "Which?", header = "Pick", options = { { label = "A" }, { label = "B" } } },
			{ question = "Which ones?", header = "Many", multi_select = true, options = { { label = "X" }, { label = "Y" } } },
		},
	}
	local function why(answers)
		return ask.check_reply(asked, { answers = answers })
	end
	t.equal(why({ { picked = { "A" } } }), "1 answers for 2 questions.")
	t.equal(why({ { picked = {} }, { picked = { "X" } } }), '"Pick" has no answer.')
	t.equal(why({ { picked = { "A" }, other = "mine" }, { picked = { "X" } } }), '"Pick" takes one answer, not 2.')
	t.equal(why({ { picked = { "A" } }, { picked = { "Z" } } }), '"Many" offers no choice "Z".')
	t.equal(why({ { picked = { "A" } }, { picked = { "Y", "X" } } }), '"Many" picks its choices more than once or out of order.')
	t.equal(why({ { picked = { "A" } }, { picked = { "X", "X" } } }), '"Many" picks its choices more than once or out of order.')
	t.equal(why({ { other = "  " }, { picked = { "X" } } }), '"Pick" has no answer.')
	t.equal(why({ { other = " mine " }, { picked = { "X", "Y" }, other = "z" } }), nil)
end)

t.case("a header's fault quotes it as written", function()
	local asked = {
		questions = {
			{
				question = "Which?",
				header = 'a "long" header\n',
				options = { { label = "A", description = "" }, { label = "B", description = "" } },
			},
		},
	}
	t.equal(ask.check(asked), "Question 1's header must be 1 to 12 characters, not \"a \\\"long\\\" header\\n\".")
	asked.questions[1].header = " \u{3000} "
	-- Unicode's white space, as Rust writes it.
	t.equal(ask.check(asked), 'Question 1\'s header must be 1 to 12 characters, not " \\u{3000} ".')
end)

t.case("keys read as the panel names them", function()
	t.equal(ask.key_of("up"), "up")
	t.equal(ask.key_of("n"), "n")
	t.equal(ask.key_of("3"), 3)
	t.equal(ask.key_of("0"), nil)
	t.equal(ask.key_of("10"), nil)
	t.equal(ask.key_of("escape"), nil)
	t.equal(ask.key_of("x"), nil)
end)
