-- Lets the agent ask the person structured questions and wait for the
-- answers (ADR 0019, ADR 0050; docs/reference/ask.md). The `ask` tool
-- takes one to four questions, each with two to four choices, one or
-- several to pick, and a preview per choice. The person can always
-- write their own answer instead, and add a note to any answer. The
-- call waits (`tau.ask`) until the answers come, the person declines,
-- or the run is cancelled; the answers are the call's result, so the
-- turn goes on without a new message.
--
-- While a call waits, its panel takes the composer's place, on the
-- computer and on phones: what the person picks, writes and notes is
-- the run's state, each key and click one of the actions below, and
-- Send gives the call its answers (`tau.answer`).
local tau = require("tau")
local ui = tau.ui
local ask = require("ask")
local panel = require("panel")

-- What the tool tells the model about itself.
local DESCRIPTION = [[Ask the person one to four questions and wait for their answers. Use it when you need a decision only they can make: a preference, a trade-off, which of several plans to follow, or something the request leaves open that you cannot settle from the code or a sensible default. Do not use it to ask whether to go on or whether a plan is ready.

Each question has a short header (at most 12 characters, shown as a tab) and two to four choices, each a short label and a description. The person can always write their own answer instead, so never offer an "Other". If you recommend a choice, put it first and end its label with "(Recommended)". Set multi_select when several choices can hold at once. A choice may carry a preview (Markdown, shown in a monospace box) when the person should compare concrete things, such as code or layouts; previews are only for questions where one choice is picked.

The result gives each question's answer and any note the person added to it, or says they declined.]]

-- The JSON Schema of what the model sends: the limits `ask.check` holds.
local PARAMETERS = {
	type = "object",
	description = "What the agent asks.",
	required = { "questions" },
	properties = {
		questions = {
			description = "One to four questions, asked together.",
			type = "array",
			minItems = 1,
			maxItems = ask.MAX_QUESTIONS,
			items = {
				description = "One question.",
				type = "object",
				required = { "question", "header", "options" },
				properties = {
					question = {
						description = "The whole question, ending in a question mark.",
						type = "string",
					},
					header = {
						description = 'A word or two for its tab, at most 12 characters: "Auth method".',
						type = "string",
						minLength = 1,
						maxLength = ask.MAX_HEADER,
					},
					options = {
						description = 'Two to four choices. The person can always write their own answer instead, so do not offer an "Other". Put a recommended choice first and end its label with "(Recommended)".',
						type = "array",
						minItems = ask.MIN_CHOICES,
						maxItems = ask.MAX_CHOICES,
						items = {
							description = "One choice.",
							type = "object",
							required = { "label", "description" },
							properties = {
								label = { description = "One to five words.", type = "string" },
								description = {
									description = "What picking it means, or what follows from it.",
									type = "string",
								},
								preview = {
									description = "Something to look at while the choice is in focus: a code snippet, a mockup, a diagram. Markdown, shown in a monospace box. Only for questions where one choice is picked.",
									type = "string",
								},
							},
						},
					},
					multi_select = {
						description = "Whether several choices can be picked.",
						type = "boolean",
						default = false,
					},
				},
			},
		},
	},
}

-- Before anything moved the keys: the panel takes them as it first
-- appears.
local function focus_start()
	return { panel = 1, other = 0, note = 0 }
end

local function calls_of(state)
	state.calls = state.calls or array({})
	return state.calls
end

-- Whether `call` still waits for the person.
local function waits(call)
	return call.reply == nil and not call.closed
end

-- The first call still waiting for the person, in the order they asked.
local function waiting(state)
	for _, call in ipairs(state.calls or {}) do
		if waits(call) then
			return call
		end
	end
	return nil
end

-- The call `id`.
local function call_of(state, id)
	for _, call in ipairs(state.calls or {}) do
		if call.call == id then
			return call
		end
	end
	return nil
end

-- Closes every call that waits: no one can answer it any more.
local function close_waiting(state)
	for _, call in ipairs(state.calls or {}) do
		if waits(call) then
			call.closed = true
		end
	end
end

-- The form the host waits on: the questions, each taking the person's
-- own answer.
local function form(questions)
	local asked = array({})
	for _, question in ipairs(questions.questions) do
		table.insert(asked, {
			question = question.question,
			header = question.header,
			options = question.options,
			multi_select = question.multi_select,
			other = true,
		})
	end
	return asked
end

-- What a key or click of the panel does, on the waiting call's draft:
-- `step(questions, draft, focus, args)` changes them and says what
-- follows, "stay", "write_other", "write_note" or "send"; nil does
-- nothing at all. What changed is kept; Send answers the call.
local function acting(step)
	return function(args, ctx)
		local call = waiting(ctx.state)
		if call == nil then
			return nil
		end
		local questions = call.ask
		local draft = call.draft or ask.draft(questions)
		local focus = call.focus or focus_start()
		local follows = step(questions, draft, focus, args or {})
		if follows == nil then
			return nil
		end
		if follows == "send" then
			local reply = ask.reply(questions, draft)
			if reply == nil then
				return nil
			end
			-- The state stays as it is: the run takes the answers.
			return tau.answer({ call = call.call, answers = reply.answers })
		end
		if follows == "write_other" then
			focus.other += 1
		elseif follows == "write_note" then
			focus.note += 1
		end
		call.draft = draft
		call.focus = focus
		return nil
	end
end

-- A row or tab a button names, from 0: what is not a whole number at
-- least 0 is the first.
local function index(value)
	if type(value) == "number" and value >= 0 and value == math.floor(value) then
		return value
	end
	return 0
end

local function text_of(args)
	return type(args.text) == "string" and args.text or ""
end

return tau.plugin({
	tools = {
		ask = {
			description = DESCRIPTION,
			parameters = PARAMETERS,
			-- A script's call would end with the script, the person still
			-- answering.
			model_only = true,
			call = function(args, ctx)
				-- A sub-agent has no one to ask: its `ask` is there so its
				-- tools match its caller's and it reads their prompt cache.
				if ctx.run.kind == "sub_agent" then
					error(ask.NO_ONE_TO_ASK, 0)
				end
				local questions, why = ask.read(args)
				if questions == nil then
					error(why, 0)
				end
				why = ask.check(questions)
				if why ~= nil then
					error(why, 0)
				end
				table.insert(calls_of(ctx.state), {
					call = ctx.run.call,
					ask = questions,
					draft = ask.draft(questions),
					focus = focus_start(),
				})
				return tau.ask({ questions = form(questions), action = "answered" })
			end,
		},
	},
	-- The calls the run's history left waiting: a run that went on from
	-- there cannot answer them any more.
	start = function(_, ctx)
		close_waiting(ctx.state)
		return nil
	end,
	-- A run that ended, cancelled too, answers nothing more.
	run_end = function(_, ctx)
		close_waiting(ctx.state)
	end,
	actions = {
		-- The keys, while the panel has them; in a field, only Esc,
		-- which gives them back to the panel.
		key = acting(function(questions, draft, focus, args)
			local name = args.key
			if args.field == true or name == "escape" then
				draft.note_open = false
				focus.panel += 1
				return "stay"
			end
			local key = ask.key_of(name)
			if key == nil then
				return nil
			end
			return ask.key(questions, draft, key)
		end),
		go = acting(function(questions, draft, _, args)
			ask.go(questions, draft, index(args.to))
			return "stay"
		end),
		choose = acting(function(questions, draft, _, args)
			return ask.choose(questions, draft, index(args.row), false)
		end),
		["other-done"] = acting(function(questions, draft, focus, args)
			ask.write_other(questions, draft, text_of(args))
			ask.other_done(questions, draft)
			focus.panel += 1
			return "stay"
		end),
		["note-done"] = acting(function(questions, draft, focus, args)
			ask.write_note(questions, draft, text_of(args))
			draft.note_open = false
			focus.panel += 1
			return "stay"
		end),
		["note-open"] = acting(function(_, draft)
			draft.note_open = true
			return "write_note"
		end),
		next = acting(function(questions, draft)
			ask.go(questions, draft, draft.tab + 1)
			return "stay"
		end),
		send = acting(function()
			return "send"
		end),
		decline = function(_, ctx)
			local call = waiting(ctx.state)
			if call == nil then
				return nil
			end
			return tau.answer({ call = call.call, declined = true })
		end,
		-- The answers reached the call: the model reads them.
		answered = function(args, ctx)
			local call = call_of(ctx.state, args.call)
			if call == nil then
				error("no call " .. tostring(args.call) .. " asked", 0)
			end
			local reply
			if args.declined then
				reply = { declined = true }
			else
				reply = { answers = args.answers }
			end
			call.reply = reply
			return ask.reply_text(call.ask, reply)
		end,
	},
	view = function(state, ctx)
		local view = {}
		local calls = {}
		for _, call in ipairs(state.calls or {}) do
			calls[call.call] = { body = panel.card(call.ask, call.reply, waits(call)) }
		end
		if next(calls) ~= nil then
			view.calls = calls
		end
		local call = waiting(state)
		if call == nil then
			return view
		end
		local n = #call.ask.questions
		view.status = ui.status(n == 1 and "1 question waiting" or (n .. " questions waiting"), nil, "warn")
		-- The run asks the person: its first question says what.
		view.asks = call.ask.questions[1].question
		view.composer = panel.tree(
			{ run = tostring(ctx.run.id), call = call.call },
			call.ask,
			call.draft or ask.draft(call.ask),
			call.focus or focus_start()
		)
		return view
	end,
})
