-- Notes what you missed (docs/reference/watcher.md, ADR 0032, 0050):
-- every 6th step of a run, a side request reads the conversation and
-- answers `learn: none`, or offers one short note the person likely
-- missed. The note is drawn under the step that asked for it and, while
-- it is new, as a line above the composer. Off until switched on.
local tau = require("tau")
local ui = tau.ui
local watch = require("watch")

-- Whether the watcher works in this run: switched on, and not for a
-- sub-agent's.
local function on(ctx)
	return ctx.settings.enabled == true and ctx.run.kind ~= "sub_agent"
end

-- The model the side request asks: the newest of the family the person
-- picked, else the chat's own.
local function family(settings)
	if settings.family == nil or settings.family == "chat" then
		return nil
	end
	return settings.family
end

-- The note's tag, in amber (grey once answered), ahead of its line.
local function tag(note, muted)
	return ui.badge(note.tag, muted and "neutral" or "warn")
end

-- The buttons a note gets: all of them while it is new, and Knew and
-- Chat once its explanation is open.
local function actions(note)
	local wanted = {}
	if note.status == "new" and note.explain then
		table.insert(wanted, { "Learn more", "learned" })
	end
	if note.status == "new" or note.status == "learned" then
		table.insert(wanted, { "Knew this already", "knew" })
		table.insert(wanted, { "Chat about it", "chatted" })
	end
	if note.status == "new" then
		table.insert(wanted, { "Dismiss", "dismissed" })
	end
	local buttons = {}
	for _, pair in ipairs(wanted) do
		table.insert(buttons, ui.button(pair[1], "answer", { key = note.key, answer = pair[2], line = note.line }))
	end
	return buttons
end

-- The note under the step that asked for it: its line, what the person
-- can do about it, and the explanation once they asked for it.
local function annotation(note)
	local muted = note.status == "knew" or note.status == "dismissed"
	local children = { ui.row({ tag(note, muted), ui.text(note.line) }) }
	if note.status == "learned" and note.explain then
		table.insert(children, ui.stack({ ui.rich("**" .. note.explain.title .. "**"), ui.list(note.explain.bullets) }))
	end
	if note.status == "knew" then
		table.insert(children, ui.text("You knew this."))
	end
	local buttons = actions(note)
	if #buttons > 0 then
		table.insert(children, ui.row(buttons))
	end
	return ui.stack(children)
end

return tau.plugin({
	name = "tau-watcher",
	description = "Every 6th step a side request reads the run and, rarely, offers one note you likely missed",
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
		view = function(settings, ctx)
			local families = { "chat" }
			for _, model in ipairs(ctx.models) do
				if model.family then
					table.insert(families, model.family)
				end
			end
			local reads = "Reads with each chat's model."
			if family(settings) then
				reads = "No " .. settings.family .. " model on the plan: reads with each chat's model."
				for _, model in ipairs(ctx.models) do
					if model.family == settings.family then
						reads = "Reads with " .. model.id .. "."
					end
				end
			end
			return ui.stack({
				ui.toggle("enabled", "Note what I missed"),
				ui.text(
					"Every 6th step, a side request reads the whole chat and almost always finds nothing. Now and then it offers one short note. Each check costs a request."
				),
				ui.rule(),
				ui.text("Model"),
				ui.text("Reads the chat and writes the notes. Chat's uses whatever model the chat runs on."),
				ui.choice("family", families),
				ui.mono(reads),
			})
		end,
	},

	-- A message written while a note waits is written past it.
	start = function(input, ctx)
		if not on(ctx) then
			return nil
		end
		local state = watch.state(ctx.state)
		if watch.unanswered(state) then
			watch.typed_past(state)
		end
		return nil
	end,

	-- The check at every 6th step: the side request reads the
	-- conversation, and its reply is a note, nothing, or dropped.
	before_request = function(request, ctx)
		if not on(ctx) then
			return nil
		end
		local state = watch.state(ctx.state)
		local steps = request.replies + 1
		-- Cheap tests first: most requests are not a 6th step, and a run
		-- notes once.
		if state.noted_in == ctx.run.id or steps % watch.EVERY ~= 0 then
			return nil
		end
		-- This run's own last check counts, whatever it found.
		local last = state.last_check
		if state.checked and state.checked.run == ctx.run.id then
			last = math.max(last or 0, state.checked.step)
		end
		if watch.band(state) ~= nil or not watch.due(steps, last, state.ignored) then
			return nil
		end
		state.checked = { run = ctx.run.id, step = steps }
		local answer = ctx.tools.infer({
			task = watch.INSTRUCTIONS
				.. "\n\n"
				.. watch.input(watch.conversation(ctx.transcript()), watch.seen(state), watch.known(state)),
			context = json.null,
			model = family(ctx.settings),
		})
		if not answer.ok then
			watch.dropped(state, steps)
			ctx.log("dropped at step " .. steps .. ": " .. tostring(answer.error))
			return nil
		end
		local reply, why = watch.parse(answer.value)
		if reply == nil then
			watch.dropped(state, steps)
			ctx.log("dropped at step " .. steps .. ": " .. why)
		elseif reply.kind == "note" then
			watch.noted(state, steps, reply)
			state.noted_in = ctx.run.id
		end
		return nil
	end,

	-- A note's buttons: the person's answer, and for a chat the note in
	-- the composer.
	actions = {
		answer = function(args, ctx)
			watch.answered(watch.state(ctx.state), args.key, args.answer)
			if args.answer == "chatted" then
				return tau.interface("composer", { text = watch.chat_text(args.line) })
			end
			return nil
		end,
	},

	view = function(state, ctx)
		state = watch.state(state)
		local notes = {}
		for _, note in ipairs(state.notes) do
			notes[note.key] = annotation(note)
		end
		local band = watch.band(state)
		local text = watch.status(state)
		return {
			notes = notes,
			band = band and ui.row({ tag(band, false), ui.text(band.line), ui.row(actions(band)) }) or nil,
			status = text and ui.status(text) or nil,
		}
	end,
})
