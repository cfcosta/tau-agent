-- Keeps a conversation going until its goal holds (ADR 0050;
-- docs/reference/goal.md). `/goal <condition>` sets it, with
-- `--continuations N` and `--budget USD` before the condition; the model
-- gets the condition and how goals work. Each time the model would
-- stop, Jev says whether the goal holds, from its last answer and its
-- recent tool results; not yet, and the model is sent back with the
-- goal, while continuations and budget last. The goal is the
-- conversation's: a resumed run keeps it, until it is met, runs out, or
-- is cleared. The banner's buttons and the `/goal` command change it.
local tau = require("tau")
local ui = tau.ui
local goal = require("goal")

local NAME = "tau-goal"

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

-- tau-goal's mark: its target, read as `tone`.
local function target(tone)
	return ui.icon("target", tone)
end

-- One of tau-goal's notes: what happened, and the small print.
local function note_tree(entry)
	return ui.row({ target(entry.tone), ui.text(entry.text), ui.mono(entry.detail) })
end

-- The person setting a goal: the condition, and the goal's limits while
-- it is the conversation's goal.
local function goal_set(g, condition)
	local children = { ui.row({ target("info"), ui.mono("/goal"), ui.rich(condition) }) }
	if g ~= nil and g.condition == condition then
		table.insert(
			children,
			ui.row({
				ui.badge("up to " .. g.max_continuations .. " continuations", "neutral"),
				ui.badge(goal.usd(g.budget) .. " budget", "neutral"),
				ui.badge("checked by Jev", "neutral"),
			})
		)
	end
	return ui.stack(children)
end

-- How a person's message tau-goal knows draws: a `/goal` as the goal it
-- set, a continuation as tau-goal's note, unless the check that sent it
-- says it already.
local function message_tree(said, state)
	if said.goal ~= nil then
		return goal_set(state.goal, said.goal)
	end
	if state.held[tostring(said.continuation)] then
		return ui.stack({})
	end
	local first = string.match(said.text, "^[^\n]*")
	first = string.gsub(string.sub(first, #goal.CONTINUATION_PREFIX + 1), "%.+$", "")
	return note_tree({ tone = "warn", text = first, detail = "before_stop" })
end

local function button(label, kind, what)
	return ui.button(label, "act", { what = what }, kind)
end

-- Above the transcript while the conversation has a goal: the goal,
-- where it stands, why nothing checks it when nothing does, and its
-- buttons.
local function banner(g, why)
	local tone = goal.tone(g)
	local actions
	if g.status == "active" then
		actions = {
			button("Pause", "secondary", "pause"),
			button("Edit", "secondary", "edit"),
			button("Clear", "secondary", "clear"),
		}
	elseif g.status == "paused" then
		actions = {
			button("Resume", "primary", "keep_going"),
			button("Edit", "secondary", "edit"),
			button("Clear", "secondary", "clear"),
		}
	elseif g.status == "met" then
		actions = { button("New goal", "secondary", "edit"), button("Clear", "secondary", "clear") }
	else
		actions = {
			button("Keep going · +" .. goal.DEFAULT_CONTINUATIONS, "primary", "keep_going"),
			button("Edit", "secondary", "edit"),
			button("Clear", "secondary", "clear"),
		}
	end
	local about = {
		ui.row({ ui.badge("goal", tone), ui.text(g.condition) }),
		ui.mono(goal.status_line(g)),
	}
	if why ~= nil then
		table.insert(about, ui.status(why, nil, "warn"))
	end
	local row = { target(tone), ui.stack(about) }
	for _, action in ipairs(actions) do
		table.insert(row, action)
	end
	return ui.row(row)
end

local STATUS = { active = "working toward it", paused = "paused", met = "met", stopped = "stopped · not met" }

-- The goal's section of the inspector: the condition, where it stands,
-- its limits, and every check.
local function section(g, live)
	local share = function(used, of)
		if of > 0 then
			return math.clamp(used / of, 0, 1)
		end
		return 1
	end
	local checks = {}
	if live and g.status == "active" then
		table.insert(checks, ui.row({ ui.icon("spinner", "info"), ui.text("working · checked when it stops") }))
	end
	for at = #g.checks, 1, -1 do
		local check = g.checks[at]
		local verdict, tone = "not met", "warn"
		if check.met then
			verdict, tone = "met", "good"
		end
		local then_
		if check.continuation ~= nil then
			then_ = "sent back, " .. check.continuation .. " of " .. g.max_continuations
		elseif check.met then
			then_ = "the run stopped"
		else
			then_ = "stopped here"
		end
		table.insert(
			checks,
			ui.row({
				ui.mono("#" .. check.n),
				ui.badge(verdict, tone),
				ui.text(then_),
				ui.mono("turn " .. check.turn .. " · p " .. string.format("%.2f", check.p)),
			})
		)
	end
	if #checks == 0 then
		table.insert(checks, ui.empty("Not checked yet."))
	end
	local cost = 0
	for _, check in ipairs(g.checks) do
		cost += check.cost
	end
	local children = {
		ui.row({ ui.heading("Goal"), ui.pill(STATUS[g.status], goal.tone(g)) }),
		ui.rich(g.condition),
		ui.text(
			"Checked when the agent stops · met at p ≥ "
				.. string.format("%.1f", goal.MET_AT)
				.. " · up to "
				.. g.max_continuations
				.. " continuations and "
				.. goal.usd(g.budget)
		),
		ui.heading("Limits"),
		ui.progress(
			share(g.continuations, g.max_continuations),
			"Continuations · " .. g.continuations .. " / " .. g.max_continuations
		),
		ui.progress(share(g.spent, g.budget), "Cost · " .. goal.usd(g.spent) .. " / " .. goal.usd(g.budget)),
		ui.heading("Checks"),
		ui.stack(checks),
	}
	if g.error ~= nil then
		table.insert(children, ui.status(g.error, nil, "bad"))
	end
	table.insert(children, ui.rows({ { "checked by", "tau-goal · Jev" }, { "Jev cost", string.format("$%.5f", cost) } }))
	table.insert(
		children,
		ui.text(
			"Jev reads the goal, the agent's last answer and its last tool results. When they fall short, the agent is sent back."
		)
	)
	return ui.stack(children)
end

-- The `/goal` popover: the command, what it does or what it needs, and
-- an input for each limit.
local function popover(jev)
	local about = jev
			and ui.text(
				"The agent keeps going until this holds. Each time it would stop, tau-goal asks Jev whether the goal is met; if not, the agent goes on."
			)
		or ui.status("Goals are checked with Jev: add a TypeSafe key on the Models screen first.", nil, "warn")
	local limit = function(id, placeholder)
		return ui.input(id, { placeholder = placeholder, submit = "tau.submit" })
	end
	return ui.stack({
		ui.row({ target("info"), ui.mono("/goal"), ui.mono("<condition>") }),
		about,
		ui.rule(),
		ui.row({
			ui.text("Stop after"),
			limit(CONTINUATIONS, tostring(goal.DEFAULT_CONTINUATIONS)),
			ui.text("continuations"),
			ui.text("Budget $"),
			limit(BUDGET, string.format("%.2f", goal.DEFAULT_BUDGET)),
		}),
		ui.rich("`Enter` set the goal and start · `Esc` close"),
	})
end

local NEEDS_JEV = "tau-goal checks goals with Jev. Add a TypeSafe key on the Models screen, then "

-- Keeps that the person's message `text` draws as `said`.
local function say(state, text, said)
	said.text = text
	state.said[text] = said
end

-- The goal set, as `/goal` writes it with its limits for a new message.
local function command_text(condition, continuations, budget)
	local line = string.match(goal.set_input(condition), "^/goal ([^\n]*)")
	return "/goal --continuations " .. continuations .. " --budget " .. string.format("%.2f", budget) .. " " .. line
end

return tau.plugin({
	name = NAME,
	description = "Keeps a conversation going until its /goal holds",
	uses = { jev = true },
	-- A goal allows 10 continuations by default, and Keep going adds 10.
	continues = 20,

	-- As a message comes in: a `/goal` sets or clears the goal, and the
	-- model is told; the run's own count of what it cost starts.
	start = function(plan, ctx)
		local state = goal.state(ctx.state)
		state.checks = ctx.jev ~= nil and ctx.run.kind ~= "sub_agent"
		state.live = ctx.run.id
		state.evidence = {}
		state.unchecked = 0
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
			return tau.start({ input = goal.CLEARED })
		end
		goal.set(state, command.condition, command.continuations, command.budget)
		local input = goal.set_input(command.condition)
		say(state, plan.input, { goal = command.condition })
		say(state, input, { goal = command.condition })
		return tau.start({ input = input })
	end,

	-- The latest tool results, for Jev.
	after_tool_result = function(result, ctx)
		local state = goal.state(ctx.state)
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

	-- What each turn cost counts toward the goal.
	turn_end = function(turn, ctx)
		local state = goal.state(ctx.state)
		state.turn = turn.turn
		state.unchecked = (state.unchecked or 0) + (turn.cost or 0)
	end,

	-- Each time the model would stop: does the goal hold? Not yet, and
	-- it goes back, while continuations and budget last.
	before_stop = function(stop, ctx)
		local state = goal.state(ctx.state)
		local g = state.goal
		if g == nil or g.status ~= "active" or not state.checks or ctx.jev == nil then
			return nil
		end
		local ok, answers, cost = pcall(ctx.jev.ask, {
			state = {
				goal = g.condition,
				final_answer = stop.text,
				recent_tool_results = state.evidence or {},
			},
			questions = { met = QUESTION },
		})
		if not ok then
			goal.failed(state, "Jev could not check the goal: " .. tostring(answers))
			return nil
		end
		cost = cost or 0
		local p = answers.met.probability
		local spent = g.spent + (state.unchecked or 0) + cost
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
			cost = cost,
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
		return tau.continue(message)
	end,

	run_end = function(run, ctx)
		local state = goal.state(ctx.state)
		state.live = nil
		state.evidence = nil
	end,

	actions = {
		-- The banner's buttons: `{ what }`, one of pause, edit, clear and
		-- keep_going.
		act = function(args, ctx)
			local state = goal.state(ctx.state)
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
				return tau.interface("composer", { text = "/goal " .. condition })
			elseif args.what == "keep_going" then
				-- Without a key nothing would check it, so it says so.
				if not ctx.run.jev then
					return tau.interface("alert", { title = "Goals need Jev", message = NEEDS_JEV .. "go on." })
				end
				if g.status == "paused" then
					goal.resume(state)
				elseif g.status == "stopped" then
					goal.extend(state, goal.DEFAULT_CONTINUATIONS)
				end
				-- A finished conversation goes on.
				if not ctx.run.live then
					return tau.interface("send", { text = goal.KEEP_GOING })
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
			run = function(text, ctx)
				local state = goal.state(ctx.state)
				local typed = "/goal " .. text
				local command = goal.parse(typed)
				if command == nil then
					-- `/goal` alone is not a goal yet: it stays, to be written.
					return tau.interface("composer", { text = "/goal " })
				end
				if command.kind == "clear" then
					goal.clear(state)
					return nil
				end
				if not ctx.run.jev then
					return tau.interface("alert", { title = "Goals need Jev", message = NEEDS_JEV .. "set the goal again." })
				end
				-- Limits typed with the command win over the popover's.
				local continuations, budget = command.continuations, command.budget
				if not (string.find(typed, "--continuations", 1, true) or string.find(typed, "--budget", 1, true)) then
					continuations, budget = goal.limits(ctx.drafts[CONTINUATIONS], ctx.drafts[BUDGET])
				end
				local condition = command.condition
				if ctx.run.live and not state.checks then
					-- A run going on that started without Jev keeps the goal
					-- for when it goes on, and nothing tells the model now.
					goal.set(state, condition, continuations, budget)
					return tau.interface("alert", {
						title = "The goal is checked from the next message",
						message = "This run started without a TypeSafe key, so nothing checks the goal while it goes on. It is kept, and checked once the conversation goes on.",
					})
				end
				if ctx.run.live then
					-- A run going on takes it at its next stop, and the model
					-- is told.
					goal.set(state, condition, continuations, budget)
					local input = goal.set_input(condition)
					say(state, input, { goal = condition })
					return tau.interface("steer", { text = input })
				end
				-- A finished conversation goes on with it; outside a chat, a
				-- new one starts.
				return tau.interface("send", { text = command_text(condition, continuations, budget) })
			end,
		},
	},

	view = function(state, ctx)
		state = goal.state(state)
		local notes = {}
		for key, entry in pairs(state.notes) do
			notes[key] = note_tree(entry)
		end
		local messages = {}
		for text, said in pairs(state.said) do
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
