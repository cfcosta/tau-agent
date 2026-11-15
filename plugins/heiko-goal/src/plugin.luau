-- Keeps a conversation going until its goal holds (ADR 0050;
-- docs/reference/goal.md). `/goal <condition>` sets it, with
-- `--continuations N` and `--budget TOKENS` before the condition; the model
-- gets the condition and how goals work. Each time the model would
-- stop, Jev says whether the goal holds, from its last answer and its
-- recent tool results; not yet, and the model is sent back with the
-- goal, while continuations and budget last. The goal is the
-- conversation's: a resumed run keeps it, until it is met, runs out, or
-- is cleared. The banner's buttons and the `/goal` command change it.
local heiko = require("heiko")
local ui = heiko.ui
local goal = require("goal")

local NAME = "heiko-goal"

-- The `/goal` popover's inputs, by id: their drafts are the limits.
local CONTINUATIONS = "goal-continuations"
local BUDGET = "goal-budget"

-- What Jev is asked at each stop.
local QUESTION = {
	kind = "noul",
	question = "Is the goal met? Judge from the tool results: the answer's own claims count only where a result backs them.",
	yes = "The tool results show that the goal holds.",
	no = "The goal does not hold yet, or nothing shown proves it.",
}

-- heiko-goal's mark, in the goal's ink.
local function target(ink)
	return ui.kanji("的", ink)
end

-- A note's or the banner's title, in the display face; `ink` is the
-- color it asks for.
local function title(text, ink)
	return ui.title(text, nil, ink)
end

local function button(label, kind, what)
	return ui.button(label, "act", { what = what }, kind)
end

-- The checks as squares, oldest first, of whether each was met: a cross
-- for one not met, a tick for the one that met it; then `last`, the
-- square after them, when there is one.
local function squares(checks, last)
	local row = {}
	for _, met in ipairs(checks) do
		if met then
			table.insert(row, ui.square("✓", "filled", "good"))
		else
			table.insert(row, ui.square("✕", "outlined", "bad"))
		end
	end
	if last then
		table.insert(row, last)
	end
	return row
end

-- Its name, small and muted: what ends a note of one line.
local function by()
	return ui.toned(NAME, "neutral", true)
end

-- One of heiko-goal's notes: the goal met or stopped under its mark with
-- its checks, a check not met or not made in a line. `stopped` is
-- whether the goal still stands stopped by this note, so it can go on
-- from here.
local function note_tree(entry, stopped)
	if entry.kind == "met" then
		return ui.row({ target("good"), ui.stack({ title("Goal met", "good"), ui.row(squares(entry.checks or {})) }) })
	elseif entry.kind == "stopped" then
		local body = {
			title("Goal stopped, not met", "bad"),
			ui.row(squares(entry.checks or {}, ui.square("■", "filled", "bad"))),
		}
		if stopped then
			table.insert(body, ui.row({ button("raise the limit", "primary", "keep_going"), button("clear", "secondary", "clear") }))
		end
		return ui.row({ target("bad"), ui.stack(body) })
	elseif entry.kind == "failed" then
		return ui.row({ ui.mark("failing"), ui.text(entry.text), ui.spacer(), ui.mono(entry.detail), by() })
	end
	return ui.row({ ui.square("✕", "outlined", "bad"), ui.text(entry.text), ui.spacer(), ui.mono(entry.detail), by() })
end

-- The person setting a goal: the condition, and how many times the goal
-- may send the agent back while it is the conversation's goal.
local function goal_set(g, condition)
	local line = { ui.mono("/goal"), ui.rich(condition) }
	if g ~= nil and g.condition == condition then
		table.insert(line, ui.spacer())
		table.insert(line, ui.stamp("up to " .. g.max_continuations .. " continuations", "neutral"))
	end
	return ui.row(line)
end

-- How a person's message heiko-goal knows draws: a `/goal` as the goal it
-- set, a continuation as heiko-goal's note, unless the check that sent it
-- says it already.
local function message_tree(said, state)
	if said.goal ~= nil then
		return goal_set(state.goal, said.goal)
	end
	if (state.held or {})[tostring(said.continuation)] then
		return ui.stack({})
	end
	local first = string.match(said.text, "^[^\n]*")
	first = string.gsub(string.sub(first, #goal.CONTINUATION_PREFIX + 1), "%.+$", "")
	return note_tree({ text = first, detail = "before_stop" }, false)
end

-- Whether each of the goal's checks was met, oldest first.
local function met_of(g)
	local out = {}
	for _, check in ipairs(g.checks) do
		table.insert(out, check.met == true)
	end
	return out
end

local STATUS = { active = "not met", paused = "paused", met = "met", stopped = "stopped · not met" }

-- Above the transcript while the conversation has a goal: the goal
-- beside its mark, each check a square, how many continuations it took
-- over them as ticks the banner's width, and its actions, then why
-- nothing checks it when nothing does.
local function banner(g, why)
	local tone = goal.tone(g)
	local heading, actions, last
	if g.status == "active" then
		heading = g.condition
		last = ui.square(tostring(#g.checks + 1))
		actions = { button("pause", "secondary", "pause"), button("edit", "secondary", "edit"), button("clear", "secondary", "clear") }
	elseif g.status == "paused" then
		heading = g.condition
		last = ui.square(tostring(#g.checks + 1))
		actions = { button("resume", "primary", "keep_going"), button("edit", "secondary", "edit"), button("clear", "secondary", "clear") }
	elseif g.status == "met" then
		heading = "Goal met"
		actions = { button("new goal", "secondary", "edit"), button("clear", "secondary", "clear") }
	else
		heading = "Goal stopped, not met"
		last = ui.square("■", "filled", "bad")
		actions = {
			button("raise the limit", "primary", "keep_going"),
			button("edit", "secondary", "edit"),
			button("clear", "secondary", "clear"),
		}
	end
	if why ~= nil then
		table.insert(actions, ui.spacer())
		table.insert(actions, ui.toned(why, "warn"))
	elseif g.error ~= nil then
		table.insert(actions, ui.spacer())
		table.insert(actions, ui.toned("the last check failed", "bad"))
	end
	local body
	if heading == g.condition then
		body = { title(heading) }
	else
		body = { title(heading, g.status == "met" and "good" or "bad"), ui.text(g.condition) }
	end
	table.insert(body, ui.row(squares(met_of(g), last)))
	-- Where it stands, when it is not just going, and the count, over
	-- the strip.
	local count = { ui.spacer(), ui.mono(g.continuations .. " of " .. g.max_continuations) }
	if g.status == "paused" then
		table.insert(count, 1, ui.toned(STATUS[g.status], tone))
	end
	table.insert(body, ui.row(count))
	table.insert(body, ui.ticks(g.continuations, g.max_continuations, tone == "bad" and "bad" or "good"))
	table.insert(body, ui.row(actions))
	return ui.row({ target(g.status == "stopped" and "bad" or "good"), ui.stack(body) })
end

-- The goal's section of the inspector: the condition, where it stands,
-- its continuations, and every check.
local function section(g, live)
	local checks = {}
	if live and g.status == "active" then
		table.insert(checks, { ui.mark("working"), ui.toned("working", "neutral"), ui.text(""), ui.text("") })
	end
	for at = #g.checks, 1, -1 do
		local check = g.checks[at]
		local then_
		if check.continuation ~= nil then
			then_ = "sent back, " .. check.continuation .. " of " .. g.max_continuations
		elseif check.met then
			then_ = "the run stopped"
		else
			then_ = "stopped here"
		end
		table.insert(checks, {
			check.met and ui.square("✓", "filled", "good") or ui.square("✕", "outlined", "bad"),
			ui.text(then_),
			ui.mono("turn " .. check.turn),
			ui.mono("p " .. string.format("%.2f", check.p)),
		})
	end
	local children = {
		ui.row({ ui.heading("Goal"), ui.spacer(), ui.toned(STATUS[g.status], goal.tone(g)) }),
		ui.rich(g.condition),
		ui.row({
			ui.toned("continuations", "neutral"),
			ui.ticks(g.continuations, g.max_continuations, "good"),
			ui.mono(g.continuations .. " of " .. g.max_continuations),
		}),
		ui.heading("Checks"),
	}
	if #checks == 0 then
		table.insert(children, ui.empty("Not checked yet."))
	else
		table.insert(children, ui.table({}, checks, { widths = { 0.6, 4, 1.5, 1.5 } }))
	end
	if g.error ~= nil then
		table.insert(children, ui.row({ ui.mark("failing"), ui.text(g.error) }))
	end
	return ui.stack(children)
end

-- The `/goal` popover: the command, what it needs when it lacks it, and
-- an input for each limit.
local function popover(jev)
	local limit = function(id, placeholder)
		return ui.input(id, { placeholder = placeholder, submit = "heiko.submit" })
	end
	local children = { ui.row({ ui.mono("/goal"), ui.mono("<condition>") }) }
	if not jev then
		table.insert(children, ui.row({ ui.mark("asks"), ui.text("Goals are checked with Jev: add a TypeSafe key (Models).") }))
	end
	table.insert(children, ui.rule())
	table.insert(
		children,
		ui.row({
			ui.toned("stop after", "neutral"),
			limit(CONTINUATIONS, tostring(goal.DEFAULT_CONTINUATIONS)),
			ui.toned("continuations", "neutral"),
			ui.toned("budget", "neutral"),
			limit(BUDGET, goal.tokens(goal.DEFAULT_BUDGET)),
		})
	)
	return ui.stack(children)
end

local NEEDS_JEV = "heiko-goal checks goals with Jev. Add a TypeSafe key on the Models screen, then "

-- Keeps that the person's message `text` draws as `said`.
local function say(state, text, said)
	said.text = text
	state.said[text] = said
end

-- The goal set, as `/goal` writes it with its limits for a new message.
local function command_text(condition, continuations, budget)
	local line = string.match(goal.set_input(condition), "^/goal ([^\n]*)")
	return "/goal --continuations " .. continuations .. " --budget " .. goal.tokens(budget) .. " " .. line
end

return heiko.plugin({
	uses = { jev = true },
	-- A goal allows 10 continuations by default, and Keep going adds 10.
	continues = 20,

	-- As a message comes in: a `/goal` sets or clears the goal, and the
	-- model is told.
	start = function(plan, model, env)
		local state = goal.state(model)
		state.checks = env.run.jev == true and env.run.kind ~= "sub_agent"
		state.live = env.run.id
		state.evidence = {}
		-- The tokens runs used that no check counted yet go on counting:
		-- a run whose stop went unchecked still spent them.
		state.unchecked = state.unchecked or 0
		state.turn = 0
		if not state.checks then
			return nil
		end
		local command = goal.parse(plan.input)
		if command == nil then
			return nil
		end
		if command.kind == "clear" then
			goal.clear(state)
			return heiko.start({ input = goal.CLEARED })
		end
		goal.set(state, command.condition, command.continuations, command.budget)
		local input = goal.set_input(command.condition)
		say(state, plan.input, { goal = command.condition })
		say(state, input, { goal = command.condition })
		return heiko.start({ input = input })
	end,

	-- The latest tool results, for Jev.
	after_tool_result = function(result, model)
		local state = goal.state(model)
		state.evidence = state.evidence or {}
		table.insert(state.evidence, {
			tool = result.name,
			arguments = result.args,
			output = goal.tail(result.text or "", goal.OUTPUT_CHARS),
		})
		while #state.evidence > goal.EVIDENCE do
			table.remove(state.evidence, 1)
		end
	end,

	-- The tokens each turn used count toward the goal.
	turn_end = function(turn, model)
		local state = goal.state(model)
		state.turn = turn.turn
		state.unchecked = (state.unchecked or 0) + (turn.tokens or 0)
	end,

	-- Each time the model would stop: does the goal hold? Not yet, and
	-- it goes back, while continuations and budget last.
	before_stop = function(stop, model, env)
		local state = goal.state(model)
		local g = state.goal
		if g == nil or g.status ~= "active" or not state.checks or not env.run.jev then
			return nil
		end
		local asked, checked = g.condition, #g.checks
		local answers, why = heiko.jev.ask({
			state = {
				goal = asked,
				final_answer = stop.text,
				recent_tool_results = state.evidence or {},
			},
			questions = { met = QUESTION },
		})
		-- The model as it is now: a Pause, a Clear or a new goal may have
		-- come while Jev answered, and then this check is not the goal's.
		state = goal.state(model)
		g = state.goal
		if g == nil or g.status ~= "active" or g.condition ~= asked or #g.checks ~= checked then
			return nil
		end
		if answers == nil then
			goal.failed(state, "Jev could not check the goal: " .. why)
			return nil
		end
		local verdict = answers.met
		if type(verdict) ~= "table" or type(verdict.probability) ~= "number" then
			goal.failed(state, "Jev gave no verdict on the goal")
			return nil
		end
		local p = verdict.probability
		local spent = g.spent + (state.unchecked or 0)
		state.unchecked = 0
		local met = p >= goal.MET_AT
		local exhausted
		if not met and g.continuations >= g.max_continuations then
			exhausted = "continuations"
		elseif not met and spent >= g.budget then
			exhausted = "budget"
		end
		local continuation = (not met and exhausted == nil) and g.continuations + 1 or nil
		local n = #g.checks + 1
		goal.check(state, {
			n = n,
			met = met,
			p = p,
			turn = state.turn or stop.turn,
			continuation = continuation,
			spent = spent,
		})
		if exhausted ~= nil then
			goal.stopped(state, exhausted)
		end
		if continuation == nil then
			return nil
		end
		local message = goal.continuation(g, n, p, continuation)
		say(state, message, { continuation = continuation })
		return heiko.continue(message)
	end,

	run_end = function(_, model)
		local state = goal.state(model)
		state.live = nil
		state.evidence = nil
	end,

	actions = {
		-- The banner's buttons: `{ what }`, one of pause, edit, clear and
		-- keep_going.
		act = function(args, model, env)
			local state = goal.state(model)
			local g = state.goal
			if g == nil then
				return nil
			end
			if args.what == "pause" then
				goal.pause(state)
			elseif args.what == "clear" then
				goal.clear(state)
			elseif args.what == "edit" then
				local condition = g.status == "met" and "" or g.condition
				return heiko.interface("composer", { text = "/goal " .. condition })
			elseif args.what == "keep_going" then
				-- Without a key nothing would check it, so it says so.
				if not env.run.jev then
					return heiko.interface("alert", { title = "Goals need Jev", message = NEEDS_JEV .. "go on." })
				end
				if g.status == "paused" then
					goal.resume(state)
				elseif g.status == "stopped" then
					goal.extend(state, goal.DEFAULT_CONTINUATIONS)
				end
				-- A finished conversation goes on.
				if not env.run.live then
					return heiko.interface("send", { text = goal.KEEP_GOING })
				end
			end
			return nil
		end,
	},

	commands = {
		goal = {
			description = "Keep working until a condition holds",
			args = "<condition>",
			icon = "target",
			popover = { jev = popover(true), no_jev = popover(false) },
			-- Sets the open conversation's goal, or a new chat's, with the
			-- popover's limits unless the command gave its own; `/goal
			-- clear` clears it.
			run = function(text, model, env)
				local state = goal.state(model)
				local typed = "/goal " .. text
				local command = goal.parse(typed)
				if command == nil then
					-- `/goal` alone is not a goal yet: it stays, to be written.
					return heiko.interface("composer", { text = "/goal " })
				end
				if command.kind == "clear" then
					goal.clear(state)
					return nil
				end
				if not env.run.jev then
					return heiko.interface("alert", { title = "Goals need Jev", message = NEEDS_JEV .. "set the goal again." })
				end
				-- Limits typed with the command win over the popover's.
				local continuations, budget = command.continuations, command.budget
				if not (string.find(typed, "--continuations", 1, true) or string.find(typed, "--budget", 1, true)) then
					continuations, budget = goal.limits(env.drafts[CONTINUATIONS], env.drafts[BUDGET])
				end
				local condition = command.condition
				if env.run.live and not state.checks then
					-- A run going on that started without Jev keeps the goal
					-- for when it goes on, and nothing tells the model now.
					goal.set(state, condition, continuations, budget)
					return heiko.interface("alert", {
						title = "The goal is checked from the next message",
						message = "This run started without a TypeSafe key, so nothing checks the goal while it goes on. It is kept, and checked once the conversation goes on.",
					})
				end
				if env.run.live then
					-- A run going on takes it at its next stop, and the model
					-- is told.
					goal.set(state, condition, continuations, budget)
					local input = goal.set_input(condition)
					say(state, input, { goal = condition })
					return heiko.interface("steer", { text = input })
				end
				-- A finished conversation goes on with it; outside a chat, a
				-- new one starts.
				return heiko.interface("send", { text = command_text(condition, continuations, budget) })
			end,
		},
	},

	-- Of a frozen copy: what `goal.state` would fill in reads as empty.
	view = function(state)
		local notes = {}
		-- The newest note is where a stopped goal goes on from.
		local newest = -1
		for key in pairs(state.notes or {}) do
			newest = math.max(newest, tonumber(string.sub(key, 2)) or -1)
		end
		local stopped = state.goal ~= nil and state.goal.status == "stopped"
		for key, entry in pairs(state.notes or {}) do
			notes[key] = note_tree(entry, stopped and key == "g" .. newest)
		end
		local messages = {}
		for text, said in pairs(state.said or {}) do
			messages[text] = message_tree(said, state)
		end
		local g = state.goal
		if g == nil then
			return { notes = notes, messages = messages }
		end
		local line = "goal · " .. g.condition
		if g.status == "met" then
			line = "goal met · " .. g.condition
		elseif g.status == "stopped" then
			line = "goal not met · " .. g.condition
		end
		return {
			notes = notes,
			messages = messages,
			status = ui.status(goal.status(g)),
			banner = banner(g, goal.unchecked(g, state.checks)),
			inspector = section(g, state.live ~= nil),
			row = {
				line = line,
				phone_line = "goal · " .. goal.status_line(g),
				count = goal.badge(g),
				icon = "target",
				tone = goal.tone(g),
			},
		}
	end,
})
