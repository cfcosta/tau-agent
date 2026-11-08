-- Picks a run's reasoning effort with Jev (ADR 0050;
-- docs/reference/plugins.md, `tau-reasoning`): once as the run starts,
-- and, with `redecide`, again when the effort's lease ends. Only runs on
-- auto are scored; an effort someone chose stands. Jev gets the task,
-- clipped at both ends, and the start of the instructions, and answers
-- one question whose levels say what each effort suits.
local tau = require("tau")
local ui = tau.ui
local effort = require("effort")

local NAME = "tau-reasoning"

-- `value` as text: a field that is null, or not text, is "".
local function text_of(value)
	return type(value) == "string" and value or ""
end

-- The plugin's mark.
local function plug(tone)
	return ui.icon("plug", tone)
end

local function usd(cost)
	return string.format("$%.4f", cost or 0)
end

-- One level a row: its name, its probability, what it suits when
-- `suits`, and a badge on the one Jev found most likely.
local function levels(choice, suits)
	local head = { "Effort", "Probability" }
	if suits then
		table.insert(head, "Suits")
	end
	table.insert(head, "")
	local rows = {}
	for _, level in ipairs(choice.levels) do
		local row = { ui.mono(level.effort), ui.progress(level.p, string.format("%.2f", level.p)) }
		if suits then
			table.insert(row, ui.text(level.suits))
		end
		table.insert(row, level.effort == choice.effort and ui.badge("most likely", "info") or ui.text(""))
		table.insert(rows, row)
	end
	return ui.table(head, rows)
end

-- The effort chart: the confidence against the threshold, then each
-- level's probability with what the level suits.
local function chart(choice)
	local verdict
	if choice.kept_for_cache then
		verdict = "is held back: it would resend " .. effort.tokens_text(choice.kept_for_cache) .. " tokens uncached"
	elseif choice.confidence >= choice.threshold then
		verdict = "is applied"
	else
		verdict = "is not applied"
	end
	return ui.stack({
		ui.rows({
			{ "Confidence", string.format("%.2f", choice.confidence) },
			{ "Threshold", string.format("%.2f", choice.threshold) },
		}),
		ui.text("So the choice " .. verdict .. "."),
		ui.rule(),
		ui.text("Probability of each effort level"),
		levels(choice, true),
	})
end

-- One choice at full size: what it was for, then its chart.
local function step(choice)
	local what = "As the message came in"
	if choice.turn ~= nil then
		what = choice.step == "user_turn" and ("Turn " .. choice.turn .. ", a message steered in")
			or ("Turn " .. choice.turn .. ", between steps")
	end
	local ran = choice.runs_at or "the model's default"
	return ui.stack({
		ui.row({ ui.mono(NAME), ui.text(what .. ": runs at " .. ran), ui.mono("Jev · " .. usd(choice.cost)) }),
		chart(choice),
	})
end

-- A note in the transcript: the effort changed, or Jev could not score.
local function note(key, entry, state)
	local details = ui.link("Details", "luau-plugins", { plugin = "tau-luau-plugins", params = { entry = NAME } })
	if entry.kind == "failed" then
		return ui.row({ plug("bad"), ui.mono(NAME), ui.text(entry.message), ui.mono(entry.detail) })
	end
	local choice = state.choices[entry.choice]
	return ui.stack({
		ui.row({ plug("info"), ui.mono(NAME), ui.rich(entry.text), ui.mono("Jev · " .. usd(choice.cost)), details }),
		ui.section("note-" .. key, "Probability of each effort level", {
			levels(choice, false),
			ui.text(effort.verdict(choice, entry.outcome)),
		}, { open = false }),
	})
end

-- Asks Jev for the effort of `state_for_jev`: the choice, or nil and why.
local function ask(ctx, levels_of, state_for_jev, redecides)
	local ok, answers, cost = pcall(ctx.jev.ask, {
		state = state_for_jev,
		questions = effort.questions(levels_of, redecides),
	})
	if not ok then
		return nil, tostring(answers)
	end
	return effort.choice(answers, levels_of, ctx.settings.threshold, cost, redecides)
end

return tau.plugin({
	name = NAME,
	description = "Picks each message's reasoning effort on auto",
	uses = { jev = true, transcript = true },
	picker_auto = {
		jev = "Auto lets tau-reasoning pick the effort for each message, with Jev.",
		no_jev = "Auto leaves the effort to the model: tau-reasoning needs a TypeSafe key (Models).",
	},
	settings = {
		schema = {
			type = "object",
			properties = {
				redecide = {
					type = "boolean",
					title = "Decide again between steps",
					description = "Jev looks again when a step's effort runs out, not only as each message comes in. Each change resends the whole context uncached, so it pays only on long tool chains.",
				},
				threshold = {
					type = "number",
					title = "Confidence to change the effort",
					description = "Below it, a message runs at the effort the last one ran at.",
				},
			},
		},
		default = { redecide = false, threshold = effort.DEFAULT_THRESHOLD },
	},

	-- As the run starts: on auto, with Jev, the effort of its message.
	start = function(input, ctx)
		local state = effort.state(ctx.state)
		state.redecides = false
		state.active = nil
		state.first = ctx.run.id
		state.effort_now = input.effort
		state.task = text_of(input.input)
		state.instructions = effort.instructions(text_of(input.instructions))
		state.levels = effort.levels(input.efforts)
		if ctx.jev == nil then
			state.starting = input.effort and ("off · effort set to " .. input.effort) or "off · no TypeSafe key"
			return nil
		end
		-- With Jev, every run leaves its context to the next message,
		-- so a long conversation keeps an effort someone chose too.
		state.active = ctx.run.id
		if input.effort ~= nil then
			state.starting = "off · effort set to " .. input.effort
			return nil
		end
		state.starting = "picks the effort as the run starts"
		-- A model that does not reason, or takes one effort, has nothing
		-- to choose.
		if #state.levels < 2 then
			return nil
		end
		state.redecides = ctx.settings.redecide == true
		local known, last = effort.last_effort(state, state.levels)
		local choice, why = ask(
			ctx,
			state.levels,
			effort.message_state(state.task, state.instructions, state.context),
			state.redecides
		)
		if choice == nil then
			-- Unsure or failed, the message goes on as the last one did.
			state.effort_now = last
			state.lease = "tool_chain"
			effort.record_error(state, "Jev could not score the task: " .. why, last, nil)
			return last and tau.start({ effort = last }) or nil
		end
		local picked = choice.verdict == "chose" and choice.effort or nil
		local runs_at, kept = effort.runs_at(picked, known, last, input.prefix_tokens or 0, effort.STICKY_TOKENS)
		choice.kept_for_cache = kept
		choice.runs_at = runs_at
		state.effort_now = runs_at
		state.lease = choice.lease
		effort.record_choice(state, choice)
		return runs_at and tau.start({ effort = runs_at }) or nil
	end,

	-- With `redecide`, before each later request whose lease ended: Jev
	-- again, on what the agent said and the results it is about to read.
	before_request = function(request, ctx)
		local state = effort.state(ctx.state)
		if state.active ~= ctx.run.id then
			return nil
		end
		state.effort_now = request.effort
		if state.first == ctx.run.id then
			state.first = nil
			return nil
		end
		if not state.redecides then
			return nil
		end
		local messages = ctx.transcript()
		if not effort.lease_ended(state.lease, state.rewritten, messages) then
			return nil
		end
		state.rewritten = nil
		local asked = effort.step_state(state.task, state.instructions, request.effort, messages)
		local choice, why = ask(ctx, state.levels, asked, true)
		if choice == nil then
			effort.record_error(state, "Jev could not score the step: " .. why, request.effort, request.turn)
			return nil
		end
		state.lease = choice.lease
		local chosen = choice.verdict == "chose" and choice.effort or nil
		local runs_at = chosen or request.effort
		choice.step = asked.step
		choice.turn = request.turn
		choice.runs_at = runs_at
		effort.record_choice(state, choice)
		state.effort_now = runs_at
		if chosen ~= nil and chosen ~= request.effort then
			return tau.effort(chosen)
		end
		return nil
	end,

	rewritten = function(info, ctx)
		local state = effort.state(ctx.state)
		if state.active == ctx.run.id then
			state.rewritten = true
		end
	end,

	-- What the message leaves the next one.
	run_end = function(run, ctx)
		local state = effort.state(ctx.state)
		if state.active ~= ctx.run.id then
			return nil
		end
		effort.record_context(state, text_of(state.task), text_of(run.text), state.effort_now)
	end,

	view = function(state, ctx)
		state = effort.state(state)
		local notes = {}
		for key, entry in pairs(state.notes) do
			notes[key] = note(key, entry, state)
		end
		local starting = effort.starting_choice(state)
		local page = { ui.heading("Reasoning effort"), ui.text("Each effort tau-reasoning picked for this run, with how sure Jev was.") }
		if #state.choices == 0 then
			table.insert(page, ui.empty("tau-reasoning has not chosen an effort in this run."))
		end
		for _, choice in ipairs(state.choices) do
			table.insert(page, step(choice))
		end
		local line = state.status or state.starting
		return {
			notes = notes,
			status = line and ui.status(line) or nil,
			plan = state.plan and { name = "reasoning", value = state.plan } or nil,
			plan_steps = starting and step(starting) or nil,
			page = ui.stack(page),
		}
	end,
})
