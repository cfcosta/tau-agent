-- Notes what you missed (docs/reference/watcher.md, ADR 0032, 0050):
-- every 6th step of a run, a side request reads the conversation and
-- answers `learn: none`, or offers one short note the person likely
-- missed. The note is drawn under the step that asked for it and, while
-- it is new, as a line above the composer. Off until switched on.
local heiko = require("heiko")
local ui = heiko.ui
local watch = require("watch")

-- Whether the watcher works in this run: switched on, and not for a
-- sub-agent's.
local function on(env)
	return env.settings.enabled == true and env.run.kind ~= "sub_agent"
end

-- The model the side request asks: the newest of the family the person
-- picked, else the chat's own.
local function family(settings)
	if settings.family == nil or settings.family == "chat" then
		return nil
	end
	return settings.family
end

-- What the person can do about a note: all of it while the note is
-- new, knowing it the primary; knowing and chatting once its
-- explanation is open.
local function actions(note)
	local wanted = {}
	if note.status == "new" or note.status == "learned" then
		table.insert(wanted, { "knew this already", "knew", "primary" })
	end
	if note.status == "new" and note.explain then
		table.insert(wanted, { "learn more", "learned" })
	end
	if note.status == "new" or note.status == "learned" then
		table.insert(wanted, { "chat about it", "chatted" })
	end
	if note.status == "new" then
		table.insert(wanted, { "dismiss", "dismissed" })
	end
	local buttons = {}
	for _, pair in ipairs(wanted) do
		table.insert(buttons, ui.button(pair[1], "answer", { key = note.key, answer = pair[2], line = note.line }, pair[3]))
	end
	return buttons
end

-- A note under the watcher's mark: its line with its tag, the
-- explanation once the person asked for it, and what they can do about
-- it. Grey once answered.
local function heads_up(note)
	local muted = note.status == "knew" or note.status == "dismissed"
	local ink = muted and "neutral" or "cyan"
	local children = { ui.row({ ui.heading(note.line), ui.spacer(), ui.stamp(note.tag, ink) }) }
	if note.status == "learned" and note.explain then
		table.insert(children, ui.stack({ ui.rich("**" .. note.explain.title .. "**"), ui.list(note.explain.bullets) }))
	end
	if note.status == "knew" then
		table.insert(children, ui.toned("You knew this.", "neutral"))
	end
	local buttons = actions(note)
	if #buttons > 0 then
		table.insert(children, ui.row(buttons))
	end
	return ui.row({ ui.kanji("報", ink), ui.stack(children) })
end

return heiko.plugin({
	uses = { infer = true, transcript = true },
	settings = {
		schema = {
			type = "object",
			properties = {
				enabled = { type = "boolean" },
				family = { type = "string" },
			},
		},
		default = { enabled = false, family = "chat" },
		view = function(settings, _, env)
			local families = { "chat" }
			for _, model in ipairs(env.models) do
				if model.family then
					table.insert(families, model.family)
				end
			end
			local reads = "Reads with each chat's model."
			if family(settings) then
				reads = "No " .. settings.family .. " model on the plan: reads with each chat's model."
				for _, model in ipairs(env.models) do
					if model.family == settings.family then
						reads = "Reads with " .. model.id .. "."
					end
				end
			end
			return ui.stack({
				ui.toggle("enabled", "Note what I missed"),
				ui.rule(),
				ui.heading("Model"),
				ui.choice("family", families),
				ui.mono(reads),
			})
		end,
	},

	-- A message written while a note waits is written past it.
	start = function(_, model, env)
		if not on(env) then
			return nil
		end
		local state = watch.state(model)
		if watch.unanswered(state) then
			watch.typed_past(state)
		end
		return nil
	end,

	-- The check at every 6th step: the side request reads the
	-- conversation, and its reply is a note, nothing, or dropped.
	before_request = function(request, model, env)
		if not on(env) then
			return nil
		end
		local run = env.run.id
		local state = watch.state(model)
		local steps = request.replies + 1
		-- Cheap tests first: most requests are not a 6th step, and a run
		-- notes once.
		if state.noted_in == run or steps % watch.EVERY ~= 0 then
			return nil
		end
		-- This run's own last check counts, whatever it found.
		local last = state.last_check
		if state.checked and state.checked.run == run then
			last = math.max(last or 0, state.checked.step)
		end
		if watch.band(state) ~= nil or not watch.due(steps, last, state.ignored) then
			return nil
		end
		state.checked = { run = run, step = steps }
		local transcript = heiko.await(heiko.cmd.transcript())
		-- The notes as they are now: the person may have answered one
		-- while the transcript came.
		state = watch.state(model)
		local ok, answer = pcall(heiko.await, heiko.cmd.infer({
			task = watch.INSTRUCTIONS
				.. "\n\n"
				.. watch.input(watch.conversation(transcript), watch.seen(state), watch.known(state)),
			context = json.null,
			model = family(env.settings),
		}))
		-- And again, after the side request.
		state = watch.state(model)
		if not ok then
			watch.dropped(state, steps)
			heiko.log("dropped at step " .. steps .. ": " .. tostring(answer))
			return nil
		end
		local reply, why = watch.parse(answer)
		if reply == nil then
			watch.dropped(state, steps)
			heiko.log("dropped at step " .. steps .. ": " .. why)
		elseif reply.kind == "note" then
			watch.noted(state, steps, reply)
			state.noted_in = run
		end
		return nil
	end,

	-- A note's buttons: the person's answer, and for a chat the note in
	-- the composer.
	actions = {
		answer = function(args, model)
			watch.answered(watch.state(model), args.key, args.answer)
			if args.answer == "chatted" then
				return heiko.interface("composer", { text = watch.chat_text(args.line) })
			end
			return nil
		end,
	},

	view = function(state)
		state = watch.state(table.clone(state))
		local notes = {}
		for _, note in ipairs(state.notes) do
			notes[note.key] = heads_up(note)
		end
		local band = watch.band(state)
		local text = watch.status(state)
		return {
			notes = notes,
			band = band and heads_up(band) or nil,
			status = text and ui.status(text) or nil,
		}
	end,
})
