-- Picks a run's reasoning effort with Jev (ADR 0050;
-- docs/reference/plugins.md, `heiko-reasoning`): once as the run starts,
-- and, with `redecide`, again when the effort's lease ends. Only runs on
-- auto are scored; an effort someone chose stands. Jev gets the task,
-- clipped at both ends, and the start of the instructions, and answers
-- one question whose levels say what each effort suits.
local heiko = require("heiko")
local ui = heiko.ui
local effort = require("effort")

local cmd = heiko.cmd

local NAME = "heiko-reasoning"

-- `value` as text: a field that is null, or not text, is "".
local function text_of(value)
	return type(value) == "string" and value or ""
end

-- How many ticks a probability is drawn on.
local TICKS = 20

-- The effort scale: each level's name outlined, the one the run goes
-- at in orange.
local function scale(choice)
	local stamps = {}
	for _, level in ipairs(choice.levels) do
		table.insert(stamps, ui.stamp(level.effort, level.effort == choice.runs_at and "warn" or "neutral"))
	end
	return ui.row(stamps)
end

-- One level a row: its name, its probability as ticks, and what it suits.
local function levels(choice)
	local rows = {}
	for _, level in ipairs(choice.levels) do
		table.insert(rows, {
			ui.mono(level.effort),
			ui.ticks(math.floor(level.p * TICKS + 0.5), TICKS, level.effort == choice.effort and "warn" or "neutral"),
			ui.mono(string.format("%.2f", level.p)),
			ui.text(level.suits),
		})
	end
	return ui.table({ "Effort", "", "p", "Suits" }, rows, { widths = { 1, 2, 1, 5 } })
end

-- What became of the choice, as a stamp.
local function verdict(choice)
	if choice.kept_for_cache then
		return ui.stamp("held · " .. heiko.count.tokens(choice.kept_for_cache) .. " cached", "warn")
	elseif choice.confidence >= choice.threshold then
		return ui.stamp("applied", "good")
	end
	return ui.stamp("not applied", "neutral")
end

-- One choice at full size: what it was for, its scale, how sure Jev was
-- against the threshold, then each level's probability.
local function step(choice)
	local what = "message"
	if choice.turn ~= nil then
		what = "turn " .. choice.turn .. (choice.step == "user_turn" and " · steered" or "")
	end
	return ui.stack({
		ui.row({
			ui.heading("Reasoning: " .. (choice.runs_at or "default")),
			ui.spacer(),
			ui.mono(what),
			verdict(choice),
		}),
		scale(choice),
		ui.row({
			ui.toned("confidence", "neutral"),
			ui.ticks(math.floor(choice.confidence * TICKS + 0.5), TICKS, choice.confidence >= choice.threshold and "good" or "neutral", { math.floor(choice.threshold * TICKS + 0.5) }),
			ui.mono(string.format("%.2f of %.2f", choice.confidence, choice.threshold)),
		}),
		levels(choice),
	})
end

-- A note in the transcript: the effort the run goes at on its scale, or
-- that Jev could not score.
local function note(entry, state)
	if entry.kind == "failed" then
		return ui.row({ ui.mark("failing"), ui.text(entry.message), ui.spacer(), ui.mono(entry.detail), ui.toned(NAME, "neutral", true) })
	end
	local choice = state.choices[entry.choice]
	return ui.stack({
		ui.row({
			ui.heading("Reasoning: " .. (choice.runs_at or "default"), "warn"),
			ui.spacer(),
			ui.toned(NAME, "neutral", true),
			ui.link("details", "luau-plugins", { plugin = "heiko-luau-plugins", params = { entry = NAME } }),
		}),
		scale(choice),
	})
end

-- Asks Jev for the effort of `state_for_jev`, of `levels_of`: the
-- choice, or nil and why. It awaits Jev: the model may have changed
-- when it returns.
local function ask(env, levels_of, state_for_jev, redecides)
	local answers, cost = heiko.jev.ask({
		state = state_for_jev,
		questions = effort.questions(levels_of, redecides),
	})
	if answers == nil then
		return nil, cost
	end
	return effort.choice(answers, levels_of, env.settings.threshold, cost, redecides)
end

return heiko.plugin({
	uses = { jev = true, transcript = true },
	picker_auto = {
		jev = "Auto lets heiko-reasoning pick the effort for each message, with Jev.",
		no_jev = "Auto leaves the effort to the model: heiko-reasoning needs a TypeSafe key (Models).",
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
	start = function(input, model, env)
		local state = effort.state(model)
		state.redecides = false
		state.active = nil
		-- What the last message chose is its own: this one says anew.
		state.status = nil
		state.plan = nil
		state.first = env.run.id
		state.effort_now = input.effort
		state.task = text_of(input.input)
		state.instructions = effort.instructions(text_of(input.instructions))
		state.levels = effort.levels(input.efforts)
		if not env.run.jev then
			state.starting = input.effort and ("off · effort set to " .. input.effort) or "off · no TypeSafe key"
			return nil
		end
		-- With Jev, every run leaves its context to the next message,
		-- so a long conversation keeps an effort someone chose too.
		state.active = env.run.id
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
		state.redecides = env.settings.redecide == true
		local choice, why = ask(
			env,
			state.levels,
			effort.message_state(state.task, state.instructions, state.context),
			state.redecides
		)
		-- The model as it is now, after Jev answered: another message may
		-- have taken it over.
		state = effort.state(model)
		if state.active ~= env.run.id then
			return nil
		end
		local known, last = effort.last_effort(state, state.levels)
		if choice == nil then
			-- Unsure or failed, the message goes on as the last one did.
			state.effort_now = last
			state.lease = "tool_chain"
			effort.record_error(state, "Jev could not score the task: " .. why, last, nil)
			return last and heiko.start({ effort = last }) or nil
		end
		local picked = choice.verdict == "chose" and choice.effort or nil
		local runs_at, kept = effort.runs_at(picked, known, last, input.prefix_tokens or 0, effort.STICKY_TOKENS)
		choice.kept_for_cache = kept
		choice.runs_at = runs_at
		state.effort_now = runs_at
		state.lease = choice.lease
		effort.record_choice(state, choice)
		return runs_at and heiko.start({ effort = runs_at }) or nil
	end,

	-- With `redecide`, before each later request whose lease ended: Jev
	-- again, on what the agent said and the results it is about to read.
	before_request = function(request, model, env)
		local state = effort.state(model)
		if state.active ~= env.run.id then
			return nil
		end
		state.effort_now = request.effort
		if state.first == env.run.id then
			state.first = nil
			return nil
		end
		if not state.redecides then
			return nil
		end
		local messages = heiko.await(cmd.transcript()) or {}
		-- The model as it is now, after the transcript came.
		state = effort.state(model)
		if state.active ~= env.run.id or not state.redecides then
			return nil
		end
		if not effort.lease_ended(state.lease, state.rewritten, messages) then
			return nil
		end
		state.rewritten = nil
		local asked = effort.step_state(state.task, state.instructions, request.effort, messages)
		local choice, why = ask(env, state.levels, asked, true)
		-- And again after Jev answered.
		state = effort.state(model)
		if state.active ~= env.run.id then
			return nil
		end
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
			return heiko.effort(chosen)
		end
		return nil
	end,

	rewritten = function(_, model, env)
		local state = effort.state(model)
		if state.active == env.run.id then
			state.rewritten = true
		end
	end,

	-- What the message leaves the next one.
	run_end = function(run, model, env)
		local state = effort.state(model)
		if state.active ~= env.run.id then
			return nil
		end
		effort.record_context(state, text_of(state.task), text_of(run.text), state.effort_now)
	end,

	-- Of a frozen copy: what `effort.state` would fill in is read as
	-- empty, not written.
	view = function(model)
		local state = { choices = model.choices or {}, notes = model.notes or {}, status = model.status, starting = model.starting, plan = model.plan }
		local notes = {}
		for key, entry in pairs(state.notes) do
			notes[key] = note(entry, state)
		end
		local starting = effort.starting_choice(state)
		local page = {}
		if #state.choices == 0 then
			table.insert(page, ui.empty("No effort picked in this run."))
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
