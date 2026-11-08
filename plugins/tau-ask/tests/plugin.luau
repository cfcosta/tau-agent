-- tau-ask in a run: a call asks and waits on its form, the panel takes
-- what the person picks, writes and notes, Send gives the call its
-- answers and the model reads them with their notes; declining tells
-- the model to go on; questions asked wrongly come back with why; a
-- sub-agent has no one to ask; a run that ends, or goes on from a
-- history that left a call waiting, closes it; and the view shows the
-- first call still waiting.
local tau = require("tau")
local t = tau.test
local g = t.gen
local ask = require("ask")

local function questions()
	return {
		questions = {
			{
				question = "How should the ask tool wait for your answer?",
				header = "Waiting",
				options = {
					{ label = "Hold the call open (Recommended)", description = "Block until the answer comes." },
					{ label = "End the turn", description = "Stop, and read the answer as the next message." },
				},
			},
			{
				question = "Where else should a waiting question show?",
				header = "Surfaces",
				multi_select = true,
				options = {
					{ label = "Run list badge", description = "A dot on the run." },
					{ label = "Notification", description = "When the window is away." },
					{ label = "Parent run", description = "For child runs." },
				},
			},
		},
	}
end

local function answered()
	return {
		{ picked = { "Hold the call open (Recommended)" }, note = "Cancel must drop the sender too." },
		{ picked = { "Run list badge", "Parent run" }, other = "the tray icon" },
	}
end

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

t.case("the call waits on its form, and the model reads the answers with their notes", function()
	local run = t.run({ id = "r1" })
	local asked = run:tool("ask", questions(), "call_1")
	-- The host waits on the form, each question taking the person's own.
	local form = asked.tau_ask
	t.equal(form.action, "answered")
	t.equal(#form.questions, 2)
	t.equal(form.questions[1].other, true)
	t.equal(form.questions[2].multi_select, true)
	t.equal(form.questions[1].options[2].label, "End the turn")
	t.equal(run.state.calls[1].call, "call_1")
	-- While it waits, the run says so and its panel takes the composer.
	local view = run:view()
	t.equal(view.status.text, "2 questions waiting")
	t.equal(view.status.tone, "warn")
	t.equal(view.asks, "How should the ask tool wait for your answer?")
	t.equal(view.composer.piece, "keys")
	-- The answers reach the call: the model reads them.
	local text = run:action("answered", { call = "call_1", answers = answered() })
	t.equal(
		text,
		'The person answered:\n"How should the ask tool wait for your answer?" = "Hold the call open (Recommended)"\n  note: Cancel must drop the sender too.\n"Where else should a waiting question show?" = "Run list badge, Parent run, the tray icon"'
	)
	t.equal(run.state.calls[1].reply, { answers = answered() })
	-- Nothing waits once answered.
	local after = run:view()
	t.equal(after.composer, nil)
	t.equal(after.status, nil)
	t.equal(after.asks, nil)
end)

t.case("declining tells the model to go on", function()
	local run = t.run({})
	run:tool("ask", questions())
	local text = run:action("answered", { call = "call_1", declined = true })
	t.equal(string.sub(text, 1, #"The person declined to answer.") == "The person declined to answer.", true)
	t.equal(run.state.calls[1].reply, { declined = true })
end)

t.case("questions asked wrongly come back with why, and nothing waits", function()
	local wrong = questions()
	wrong.questions[1].options[2].label = "Other"
	local run = t.run({})
	local ok, why = pcall(function()
		return run:tool("ask", wrong)
	end)
	t.equal(ok, false)
	t.equal(string.find(why, '"Other"', 1, true) ~= nil, true, why)
	t.equal(run.state.calls, nil)
	-- Ones that do not read as questions say what is missing.
	local _, missing = pcall(function()
		return run:tool("ask", { questions = { { question = "Which?", options = {} } } })
	end)
	t.equal(missing, "questions[0]: `header` is not a string")
end)

t.case("a sub-agent has no one to ask, and every other run does", function()
	for _, kind in ipairs({ "main", "chat", "sub_agent" }) do
		local run = t.run({ kind = kind })
		local ok, answer = pcall(function()
			return run:tool("ask", { questions = {} })
		end)
		t.equal(ok, false)
		t.equal(answer == ask.NO_ONE_TO_ASK, kind == "sub_agent", kind)
	end
end)

t.case("a run that ends, cancelled or not, closes the call that waits", function()
	local run = t.run({})
	run:tool("ask", questions())
	run:run_end({ stop = "Cancelled", turns = 1, cost = 0, text = "" })
	t.equal(run.state.calls[1].closed, true)
	t.equal(run:view().composer, nil)
	-- Its card shows what it asked, waiting for no one.
	local body = run:view().calls.call_1.body
	t.equal(#body.children, 1)
end)

t.case("a run that goes on closes what its history left waiting", function()
	local run = t.run({ state = { calls = { { call = "call_9", ask = questions() } } } })
	t.equal(run:view().asks, questions().questions[1].question)
	run:start({ input = "go on" })
	t.equal(run.state.calls[1].closed, true)
	t.equal(run:view().asks, nil)
end)

t.case("only the model calls ask, with the limits the check holds", function()
	local plugin = t.load()
	local tool = plugin.tools.ask
	t.equal(tool.model_only, true)
	local schema = tool.parameters.properties.questions
	t.equal({ schema.minItems, schema.maxItems }, { 1, ask.MAX_QUESTIONS })
	local question = schema.items.properties
	t.equal({ question.header.minLength, question.header.maxLength }, { 1, ask.MAX_HEADER })
	t.equal({ question.options.minItems, question.options.maxItems }, { ask.MIN_CHOICES, ask.MAX_CHOICES })
end)

-- The waiting call's draft, as the run keeps it.
local function draft_of(run)
	return run.state.calls[1].draft
end

local function key(run, name, field)
	return run:action("key", { key = name, field = field == true })
end

t.case("the panel takes the keys, and Send gives the call what was picked, written and noted", function()
	local asked = questions()
	asked.questions[1].options[1].preview = "select! { a = rx => a }"
	asked.questions[1].options[2].preview = "stop_run()"
	local run = t.run({ id = "r1", live = true })
	run:tool("ask", asked, "call_ask")
	t.equal(run.state.calls[1].focus.panel, 1, "the panel takes the keys as it appears")
	-- `2` picks the second choice and goes on; on the checklist `1` and
	-- `3` toggle, and `n` opens the note, which takes text and closes.
	t.equal(key(run, "2"), nil)
	t.equal(draft_of(run).tab, 1)
	key(run, "1")
	key(run, "3")
	key(run, "n")
	t.equal(draft_of(run).note_open, true)
	t.equal(run.state.calls[1].focus.note, 1, "the note's field takes the keys")
	run:action("note-done", { text = "only for long waits" })
	t.equal(draft_of(run).note_open, false)
	t.equal(run.state.calls[1].focus.panel, 2, "back on the panel")
	-- Right is the review, and Enter sends, leaving the state as it is.
	key(run, "right")
	local before = copy(run.state)
	local sent = key(run, "enter")
	t.equal(run.state, before, "sending changes nothing")
	t.equal(sent, tau.answer({
		call = "call_ask",
		answers = {
			{ picked = { "End the turn" } },
			{ picked = { "Run list badge", "Parent run" }, note = "only for long waits" },
		},
	}))
end)

t.case("the person's own answer goes on to the next question, and Esc gives the keys back", function()
	local run = t.run({ live = true })
	run:tool("ask", questions())
	-- The third row is the person's own: its field takes the keys.
	key(run, "3")
	t.equal(run.state.calls[1].focus.other, 1)
	run:action("other-done", { text = "  ask me later  " })
	t.equal(draft_of(run).tab, 1, "written, a one-answer question goes on")
	-- Esc in a field: back to the panel.
	key(run, "escape", true)
	t.equal(run.state.calls[1].focus.panel, 3)
	run:action("choose", { row = 1 })
	run:action("go", { to = 2 })
	local sent = run:action("send", {})
	t.equal(sent.tau_answer.answers, { { picked = {}, other = "ask me later" }, { picked = { "Notification" } } })
	-- Not ready, Send sends nothing.
	local fresh = t.run({})
	fresh:tool("ask", questions())
	t.equal(fresh:action("send", {}), nil)
	t.equal(fresh:action("decline", {}), tau.answer({ call = "call_1", declined = true }))
end)

t.case("the panel draws the question in view, its preview, and the review", function()
	local asked = questions()
	asked.questions[1].options[1].preview = "select! { a = rx => a }"
	local run = t.run({ id = "r1" })
	run:tool("ask", asked, "c1")
	local tree = run:view().composer
	t.equal(tree.id, "ask-panel")
	t.equal(tree.through, { "escape" })
	local card = tree.children[1]
	local tabs, question, foot = card.children[1], card.children[2], card.children[3]
	t.equal(tabs.children[1].label, "Waiting")
	t.equal(tabs.children[1].kind, "primary")
	t.equal(tabs.children[#tabs.children].text, "pick one")
	-- The rows, the person's own field, and the preview of the row in
	-- focus, its description beside it.
	t.equal(question.children[1].text, "**How should the ask tool wait for your answer?**")
	t.equal(question.children[2].children[1].text, "› 1")
	t.equal(#question.children[2].children, 2, "previews take the descriptions")
	t.equal(question.children[4].children[2].id, "ask-other-r1-c1-0")
	t.equal(question.children[5].children[2].text, "select! { a = rx => a }")
	t.equal(foot.children[3].action, "tau.cancel")
	-- The review: each answer, or that there is none, with a way back.
	run:action("key", { key = "1" })
	run:action("key", { key = "right" })
	tree = run:view().composer
	local review = tree.children[1].children[2]
	t.equal(review.piece, "table")
	t.equal(review.rows[1][2].text, "Hold the call open (Recommended)")
	t.equal(review.rows[2][2].text, "not answered")
	local send = tree.children[1].children[3].children[5]
	t.equal(send.label, "Send answers · 1 left")
	t.equal(send.kind, "secondary")
end)

t.case("the card shows each answer with its note", function()
	local run = t.run({})
	run:tool("ask", questions())
	local waiting = run:view().calls.call_1.body
	t.equal(waiting.children[2].text, "Answer in the panel below.")
	run:action("answered", { call = "call_1", answers = answered() })
	local body = run:view().calls.call_1.body
	local rows = body.children[1].rows
	t.equal(rows[1][1].text, "Waiting")
	t.equal(rows[1][2].children[1].text, "**Hold the call open (Recommended)**")
	t.equal(rows[1][2].children[2].children[2].text, "Cancel must drop the sender too.")
	t.equal(rows[2][2].children[1].text, "**Run list badge, Parent run, the tray icon**")
	t.equal(#body.children, 1)
	local declined = t.run({})
	declined:tool("ask", questions())
	declined:action("answered", { call = "call_1", declined = true })
	local said = declined:view().calls.call_1.body
	t.equal(said.children[2].text, "You declined to answer.")
end)

-- Over any calls that ask, answers and closes, the view shows the first
-- call, in the order they asked, still waiting; and it says so.
t.machine("the view shows the first call still waiting", {
	init = function()
		return { run = t.run({ id = "r1" }), calls = {}, n = 0 }
	end,
	rules = {
		ask = function(m)
			m.n += 1
			local id = "call_" .. m.n
			m.run:tool("ask", questions(), id)
			table.insert(m.calls, { id = id, waits = true })
		end,
		answer = function(m, draw)
			t.assume(#m.calls > 0)
			local at = draw(g.integers({ min = 1, max = #m.calls }))
			local declined = draw(g.booleans())
			local call = m.calls[at]
			if declined then
				m.run:action("answered", { call = call.id, declined = true })
			else
				m.run:action("answered", { call = call.id, answers = answered() })
			end
			call.waits = false
		end,
		ends = function(m)
			m.run:run_end({ stop = "Stop", turns = 1, cost = 0, text = "" })
			for _, call in ipairs(m.calls) do
				call.waits = false
			end
		end,
	},
	invariant = function(m)
		local first = nil
		for _, call in ipairs(m.calls) do
			if call.waits then
				first = call.id
				break
			end
		end
		local view = m.run:view()
		t.equal(view.composer ~= nil, first ~= nil, "a panel while one waits")
		t.equal(view.asks ~= nil, first ~= nil, "asks while one waits")
		if first ~= nil then
			local id = view.composer.children[1].children[2].children[4].children[2].id
			t.equal(id, "ask-other-r1-" .. first .. "-0")
		end
		for _, call in ipairs(m.calls) do
			t.equal(view.calls[call.id] ~= nil, true, "every call has its card")
		end
	end,
})
