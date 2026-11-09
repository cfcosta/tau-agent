-- tau-schedule: a chat wakes itself later (ADR 0039).
--
-- A chat's `schedule` tool leaves it a note for a time to come; when that
-- time comes, the chat takes a turn of tau's on the note. Once only:
-- repeating is for agents (ADR 0037), whose own `schedule` takes this
-- one's place in their runs. Waking is at least once: a wake-up leaves
-- the store only once its turn started, so tau closing before then wakes
-- the chat when it starts again, and closing as the turn starts may wake
-- it once more on the same note.
--
-- What a chat has waiting is in the repository's store, which the job
-- that wakes chats reads each second, and in the chat's state, which its
-- inspector draws: each turn's `start` reads the store again.
local tau = require("tau")
local ui = tau.ui
local wake = require("wake")

-- What the chat `run` has waiting, soonest first, from `store`.
local function waiting(store, run)
	local list = {}
	for _, item in ipairs(store.list(wake.prefix(run))) do
		table.insert(list, item.value)
	end
	return wake.sort(list)
end

-- The chat's state again from what the store holds of it.
local function refresh(ctx)
	ctx.state.waiting = waiting(ctx.store, ctx.run.id)
end

-- `at` when the model gave `in` or `on`, or why not.
local function when(args, ctx)
	local now = ctx.now.unix
	if args["in"] ~= nil and args.on == nil then
		local seconds = ctx.time.duration(args["in"])
		if seconds == nil then
			error("`" .. tostring(args["in"]) .. '` is not a duration, like "2h"', 0)
		end
		return now + seconds
	elseif args.on ~= nil and args["in"] == nil then
		local at = ctx.time.parse(args.on)
		if at == nil then
			error(
				"`"
					.. tostring(args.on)
					.. '` is not a local date and time, like "'
					.. ctx.time["local"](now + 86400)
					.. '"',
				0
			)
		end
		return at
	end
	error('Say when with one of `in` ("2h") or `on` (a local date and time)', 0)
end

-- The fields `schedule` takes; `every`, `at` and `days` are read only
-- to say why they are refused.
local FIELDS = { name = true, note = true, ["in"] = true, on = true }

local function schedule(args, ctx)
	if ctx.run.kind == "sub_agent" then
		error(wake.SUB_AGENTS, 0)
	end
	for field in pairs(args) do
		if not FIELDS[field] and field ~= "every" and field ~= "at" and field ~= "days" then
			error("unknown field `" .. tostring(field) .. "`", 0)
		end
	end
	local name = string.match(tostring(args.name or ""), "^%s*(.-)%s*$")
	if not wake.valid_name(name) then
		error(
			"`" .. name .. "` cannot name a wake-up: lower-case letters, digits, `-` and `_`, starting with a letter, at most 40",
			0
		)
	end
	local note = string.match(tostring(args.note or ""), "^%s*(.-)%s*$")
	if note == "" then
		error("Leave yourself a note: what to do then", 0)
	end
	if args.every ~= nil or args.at ~= nil or args.days ~= nil then
		error(wake.NO_REPEATS, 0)
	end
	local now = ctx.now.unix
	local at = when(args, ctx)
	if at < now + wake.SOONEST then
		error("That is " .. ctx.time["local"](at) .. ": a minute from now is the soonest", 0)
	end
	if at > now + wake.FURTHEST then
		error("A year ahead is the furthest", 0)
	end
	local run = ctx.run.id
	local key = wake.key(run, name)
	local replaced = ctx.store.get(key) ~= nil
	if not replaced and #waiting(ctx.store, run) >= wake.MAX_WAITING then
		error("This chat has " .. wake.MAX_WAITING .. " waiting, the most it may: cancel some first", 0)
	end
	ctx.store.put(key, { run = run, name = name, note = note, at = at, made = now })
	refresh(ctx)
	return (replaced and "Replaced" or "Scheduled")
		.. " `"
		.. name
		.. "` for "
		.. ctx.time["local"](at)
		.. ". This chat wakes then with your note."
end

local function scheduled(_, ctx)
	local list = waiting(ctx.store, ctx.run.id)
	local now = ctx.time["local"](ctx.now.unix)
	if #list == 0 then
		return "It is " .. now .. ". Nothing is scheduled."
	end
	local lines = { "It is " .. now .. ". " .. #list .. " of the " .. wake.MAX_WAITING .. " this chat may have:" }
	for _, one in ipairs(list) do
		local first = string.match(one.note, "^[^\n]*")
		table.insert(lines, "- " .. one.name .. " · " .. ctx.time["local"](one.at) .. " · " .. first)
	end
	return table.concat(lines, "\n")
end

-- Cancels what the chat scheduled as `name`: whether there was one.
local function cancel(name, ctx)
	local key = wake.key(ctx.run.id, string.match(tostring(name), "^%s*(.-)%s*$"))
	local had = ctx.store.get(key) ~= nil
	ctx.store.delete(key)
	refresh(ctx)
	return had
end

local function unschedule(args, ctx)
	local name = tostring(args.name)
	if cancel(name, ctx) then
		return "Cancelled `" .. name .. "`."
	end
	return "Nothing is scheduled as `" .. name .. "`."
end

-- One wake-up: its name, when it comes due, the button that cancels it,
-- and its note.
local function row(run, one)
	return ui.stack({
		ui.row({
			ui.icon("clock", "neutral"),
			ui.text(one.name),
			ui.spacer(),
			ui.due(run .. "-" .. one.name, one.at * 1000),
			ui.button("", "cancel", { name = one.name }, "secondary", "close"),
		}),
		ui.text(one.note),
	})
end

-- The message a chat woke on, as a card: each wake-up that came due, its
-- name, when, and its note.
local function card(woken)
	local blocks = {}
	for index, one in ipairs(woken) do
		if index > 1 then
			table.insert(blocks, ui.rule())
		end
		table.insert(
			blocks,
			ui.stack({
				ui.row({
					ui.icon("clock", "info"),
					ui.text("Scheduled · " .. one.name),
					ui.spacer(),
					ui.mono(one.due),
				}),
				ui.text(one.note),
			})
		)
	end
	return ui.card({ ui.stack(blocks) })
end

-- Wakes each chat in `store` that has something due at `now` and may
-- take a turn: `retry` says when a chat whose turn did not start is
-- tried again.
local function wake_due(store, now, retry, ctx)
	local due, runs = {}, {}
	for _, item in ipairs(store.list("wake/")) do
		local one = item.value
		if one.at <= now and (retry[one.run] or 0) <= now then
			if due[one.run] == nil then
				due[one.run] = {}
				table.insert(runs, one.run)
			end
			table.insert(due[one.run], one)
		end
	end
	table.sort(runs)
	for _, run in ipairs(runs) do
		local list = wake.sort(due[run])
		local woken = {}
		for _, one in ipairs(list) do
			table.insert(woken, { name = one.name, due = ctx.time["local"](one.at), note = one.note })
		end
		local ok, answer = pcall(ctx.chats.wake, run, wake.message(woken))
		if not ok then
			ctx.log("could not wake " .. run .. ": " .. tostring(answer))
			retry[run] = now + wake.RETRY
		elseif answer == "started" then
			retry[run] = nil
			for _, one in ipairs(list) do
				-- One scheduled again meanwhile stays.
				local kept = store.get(wake.key(run, one.name))
				if kept ~= nil and kept.at == one.at and kept.made == one.made and kept.note == one.note then
					store.delete(wake.key(run, one.name))
				end
			end
		elseif answer == "gone" then
			-- A chat that landed or was dropped takes no turn, and what it
			-- had waiting goes.
			retry[run] = nil
			for _, item in ipairs(store.list(wake.prefix(run))) do
				store.delete(item.key)
			end
		end
	end
end

return tau.plugin({
	uses = { store = true, chats = true },

	tools = {
		schedule = {
			description = "Wake this chat later, once, with a note to yourself for then: `in` a while, or `on` a local date and time. When it comes, this chat takes a turn whose message is your note, after any turn going ends. Use it when work waits on something: a review, a release, a long build. Scheduling a name again replaces it. The person sees what you scheduled beside the chat and may cancel it.",
			parameters = {
				type = "object",
				properties = {
					name = {
						type = "string",
						description = "Lower-case letters, digits, `-` and `_`: `recheck-pr-412`.",
					},
					note = {
						type = "string",
						description = "What to do then, for you to read when it comes.",
					},
					["in"] = { type = "string", description = "After this long: `90m`, `2h`, `3d`." },
					on = { type = "string", description = "At this local date and time: `2026-10-13 09:00`." },
				},
				required = { "name", "note" },
				additionalProperties = false,
			},
			call = schedule,
		},
		scheduled = {
			description = "What this chat has scheduled with `schedule` that is still to come, soonest first, and the host's local time now. Look before you schedule something again.",
			parameters = { type = "object", properties = {}, additionalProperties = false },
			call = scheduled,
		},
		unschedule = {
			description = "Cancel what this chat scheduled as `name`.",
			parameters = {
				type = "object",
				properties = { name = { type = "string" } },
				required = { "name" },
				additionalProperties = false,
			},
			call = unschedule,
		},
	},

	-- Each turn: what the chat has waiting, as the store has it now, and
	-- the card of the message it woke on, if it did.
	start = function(plan, ctx)
		refresh(ctx)
		local woken = type(plan.input) == "string" and wake.parse(plan.input) or nil
		if woken ~= nil then
			ctx.state.cards = ctx.state.cards or {}
			ctx.state.cards[plan.input] = woken
		end
		return nil
	end,

	-- The person cancelled a wake-up.
	actions = {
		cancel = function(args, ctx)
			if not cancel(args.name, ctx) then
				error("Nothing is scheduled as " .. tostring(args.name) .. " in this chat", 0)
			end
		end,
	},

	-- Each second, every chat with a wake-up due takes a turn of tau's on
	-- the notes of all that came due, unless a turn of it is going: then
	-- they wait for it to end.
	jobs = {
		wake = {
			every = 1,
			label = "Waking chats",
			run = function(ctx)
				local now = ctx.now.unix
				local retry = ctx.state.retry or {}
				for run, at in pairs(retry) do
					if at <= now then
						retry[run] = nil
					end
				end
				for _, repo in ipairs(ctx.repos()) do
					wake_due(ctx.stores(repo), now, retry, ctx)
				end
				ctx.state.retry = retry
			end,
		},
	},

	-- What the chat has waiting, in its inspector, with a way to cancel
	-- each; and the messages it woke on, as cards.
	view = function(state, ctx)
		local view = {}
		local list = state.waiting or {}
		if #list > 0 then
			local rows = { ui.heading("Scheduled") }
			for _, one in ipairs(list) do
				table.insert(rows, row(ctx.run.id, one))
			end
			view.inspector = ui.stack(rows)
		end
		if state.cards ~= nil and next(state.cards) ~= nil then
			view.messages = {}
			for text, woken in pairs(state.cards) do
				view.messages[text] = card(woken)
			end
		end
		return view
	end,
})
