-- heiko-schedule: a chat wakes itself later (ADR 0039).
--
-- A chat's `schedule` tool leaves it a note for a time to come; when that
-- time comes, the chat takes a turn of heiko's on the note. Once only:
-- repeating is for agents (ADR 0037), whose own `schedule` takes this
-- one's place in their runs. Waking is at least once: a wake-up leaves
-- the store only once its turn started, so heiko closing before then wakes
-- the chat when it starts again, and closing as the turn starts may wake
-- it once more on the same note.
--
-- What a chat has waiting is in the repository's store, which the job
-- that wakes chats reads each second, and in the chat's state, which its
-- inspector draws: each turn's `start` reads the store again.
local heiko = require("heiko")
local ui = heiko.ui
local wake = require("wake")
local cmd = heiko.cmd

-- The host's local time of `at`.
local function local_time(at)
	return heiko.await(cmd.time["local"](at))
end

-- What the chat `run` has waiting, soonest first, from `store`.
local function waiting(store, run)
	local list = {}
	for _, item in ipairs(store.list(wake.prefix(run))) do
		table.insert(list, item.value)
	end
	return wake.sort(list)
end

-- The chat's state again from what the store holds of it. A turn woken
-- on `woken` leaves out what woke it, though due: the job that woke it
-- lets go of those once the turn started, after this.
local function refresh(model, env, woken)
	local gone = {}
	for _, one in ipairs(woken or {}) do
		gone[one.name] = true
	end
	local list = {}
	for _, one in ipairs(waiting(heiko.store, env.run.id)) do
		if not (gone[one.name] and one.at <= env.now.unix) then
			table.insert(list, one)
		end
	end
	model.waiting = list
end

-- `at` when the model gave `in` or `on`, or why not.
local function when(args, env)
	local now = env.now.unix
	if args["in"] ~= nil and args.on == nil then
		local seconds = heiko.await(cmd.time.duration(args["in"]))
		if seconds == nil then
			error("`" .. tostring(args["in"]) .. '` is not a duration, like "2h"', 0)
		end
		return now + seconds
	elseif args.on ~= nil and args["in"] == nil then
		local at = heiko.await(cmd.time.parse(args.on))
		if at == nil then
			error(
				"`"
					.. tostring(args.on)
					.. '` is not a local date and time, like "'
					.. local_time(now + 86400)
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

local function schedule(args, model, env)
	if env.run.kind == "sub_agent" then
		error(wake.SUB_AGENTS, 0)
	end
	for field in pairs(args) do
		if not FIELDS[field] and field ~= "every" and field ~= "at" and field ~= "days" then
			error("unknown field `" .. tostring(field) .. "`", 0)
		end
	end
	local name = heiko.text.trim(tostring(args.name or ""))
	if not wake.valid_name(name) then
		error(
			"`" .. name .. "` cannot name a wake-up: lower-case letters, digits, `-` and `_`, starting with a letter, at most 40",
			0
		)
	end
	local note = heiko.text.trim(tostring(args.note or ""))
	if note == "" then
		error("Leave yourself a note: what to do then", 0)
	end
	if args.every ~= nil or args.at ~= nil or args.days ~= nil then
		error(wake.NO_REPEATS, 0)
	end
	local now = env.now.unix
	local at = when(args, env)
	if at < now + wake.SOONEST then
		error("That is " .. local_time(at) .. ": a minute from now is the soonest", 0)
	end
	if at > now + wake.FURTHEST then
		error("A year ahead is the furthest", 0)
	end
	local run = env.run.id
	local key = wake.key(run, name)
	local replaced = heiko.store.get(key) ~= nil
	if not replaced and #waiting(heiko.store, run) >= wake.MAX_WAITING then
		error("This chat has " .. wake.MAX_WAITING .. " waiting, the most it may: cancel some first", 0)
	end
	heiko.store.put(key, { run = run, name = name, note = note, at = at, made = now })
	refresh(model, env)
	return heiko.reply(
		(replaced and "Replaced" or "Scheduled") .. " `" .. name .. "` for " .. local_time(at) .. ". This chat wakes then with your note.",
		{ name = name, note = note, at = local_time(at), replaced = replaced }
	)
end

local function scheduled(_, _, env)
	local list = waiting(heiko.store, env.run.id)
	local now = local_time(env.now.unix)
	if #list == 0 then
		return heiko.reply("It is " .. now .. ". Nothing is scheduled.", { waiting = {} })
	end
	local lines = { "It is " .. now .. ". " .. #list .. " of the " .. wake.MAX_WAITING .. " this chat may have:" }
	local waiting_ = {}
	for _, one in ipairs(list) do
		local first = string.match(one.note, "^[^\n]*")
		table.insert(lines, "- " .. one.name .. " · " .. local_time(one.at) .. " · " .. first)
		table.insert(waiting_, { key = one.name, value = local_time(one.at) .. " · " .. first })
	end
	return heiko.reply(table.concat(lines, "\n"), { waiting = waiting_ })
end

-- Cancels what the chat scheduled as `name`: whether there was one.
local function cancel(name, model, env)
	local key = wake.key(env.run.id, heiko.text.trim(tostring(name)))
	local had = heiko.store.get(key) ~= nil
	heiko.store.delete(key)
	refresh(model, env)
	return had
end

local function unschedule(args, model, env)
	local name = tostring(args.name)
	if cancel(name, model, env) then
		return heiko.reply("Cancelled `" .. name .. "`.", { name = name, cancelled = true })
	end
	return heiko.reply("Nothing is scheduled as `" .. name .. "`.", { name = name, cancelled = false })
end

-- Each tool's step (ADR 0064): the wake-up's name, what came of the
-- call, and the note or the list under it.
local steps = {}

function steps.schedule(input)
	local made = input.result
	return {
		subject = made.name,
		receipt = { ui.receipt.stamp(made.replaced and "replaced" or "scheduled", "good"), ui.receipt.measure(made.at) },
		body = ui.form.record({ { key = "note", value = made.note } }),
	}
end

function steps.scheduled(input)
	local list = input.result.waiting
	if #list == 0 then
		return { receipt = { ui.receipt.measure("nothing scheduled") } }
	end
	return { receipt = { ui.receipt.count(#list, "wake-up") }, body = ui.form.record(list) }
end

function steps.unschedule(input)
	local done = input.result
	return {
		subject = done.name,
		receipt = { done.cancelled and ui.receipt.stamp("cancelled", "good") or ui.receipt.measure("not scheduled") },
	}
end

-- One wake-up: its name, when it comes due and the way to cancel it;
-- under them its note, dimmed.
local function row(run, one)
	return ui.stack({
		ui.row({
			ui.mono(one.name),
			ui.spacer(),
			ui.due(run .. "-" .. one.name, one.at * 1000),
			ui.button("cancel", "cancel", { name = one.name }, "secondary"),
		}),
		ui.toned(one.note, "neutral"),
	})
end

-- The message a chat woke on: each wake-up that came due, the time it
-- was for as a figure beside its name, that it was scheduled and for
-- which day, and its note.
local function card(woken)
	local blocks = {}
	for index, one in ipairs(woken) do
		if index > 1 then
			table.insert(blocks, ui.rule())
		end
		local day, time = string.match(one.due, "^(.-)%s*(%d+:%d+)$")
		local said = "scheduled"
		if day ~= nil and day ~= "" then
			said ..= " · " .. day
		end
		table.insert(
			blocks,
			ui.row({
				ui.figure(time or one.due),
				ui.stack({
					ui.row({ ui.title(one.name), ui.spacer(), ui.toned(said, "neutral", true) }),
					ui.text(one.note),
				}),
			})
		)
	end
	return ui.stack(blocks)
end

-- Wakes each chat in `store` that has something due at `now` and may
-- take a turn: `model.retry` says when a chat whose turn did not start
-- is tried again. It is read again after each wake, which another step
-- may have changed meanwhile.
local function wake_due(store, now, model)
	local due, runs = {}, {}
	local retry = model.retry or {}
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
			table.insert(woken, { name = one.name, due = local_time(one.at), note = one.note })
		end
		local ok, answer = pcall(heiko.await, cmd.chats.wake(run, wake.message(woken)))
		model.retry = model.retry or {}
		if not ok then
			heiko.log("could not wake " .. run .. ": " .. tostring(answer))
			model.retry[run] = now + wake.RETRY
		elseif answer == "started" then
			model.retry[run] = nil
			for _, one in ipairs(list) do
				-- One scheduled again meanwhile stays.
				local key = wake.key(run, one.name)
				local kept = store.get(key)
				if kept ~= nil and kept.at == one.at and kept.made == one.made and kept.note == one.note then
					store.delete(key)
				end
			end
		elseif answer == "gone" then
			-- A chat that landed or was dropped takes no turn, and what it
			-- had waiting goes.
			model.retry[run] = nil
			for _, item in ipairs(store.list(wake.prefix(run))) do
				store.delete(item.key)
			end
		end
	end
end

return heiko.plugin({
	uses = { store = true, chats = true },

	tools = {
		schedule = {
			kind = "change",
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
			subject = "name",
			step = steps.schedule,
		},
		scheduled = {
			kind = "look",
			description = "What this chat has scheduled with `schedule` that is still to come, soonest first, and the host's local time now. Look before you schedule something again.",
			parameters = { type = "object", properties = {}, additionalProperties = false },
			call = scheduled,
			step = steps.scheduled,
		},
		unschedule = {
			kind = "change",
			description = "Cancel what this chat scheduled as `name`.",
			parameters = {
				type = "object",
				properties = { name = { type = "string" } },
				required = { "name" },
				additionalProperties = false,
			},
			call = unschedule,
			subject = "name",
			step = steps.unschedule,
		},
	},

	-- Each turn: what the chat has waiting, as the store has it now, and
	-- the card of the message it woke on, if it did.
	start = function(plan, model, env)
		local woken = type(plan.input) == "string" and wake.parse(plan.input) or nil
		refresh(model, env, woken)
		if woken ~= nil then
			model.cards = model.cards or {}
			model.cards[plan.input] = woken
		end
		return nil
	end,

	-- The person cancelled a wake-up.
	actions = {
		cancel = function(args, model, env)
			if not cancel(args.name, model, env) then
				error("Nothing is scheduled as " .. tostring(args.name) .. " in this chat", 0)
			end
		end,
	},

	-- Each second, every chat with a wake-up due takes a turn of heiko's on
	-- the notes of all that came due, unless a turn of it is going: then
	-- they wait for it to end.
	jobs = {
		wake = {
			every = 1,
			label = "Waking chats",
			run = function(_, model, env)
				local now = env.now.unix
				local retry = model.retry or {}
				for run, at in pairs(retry) do
					if at <= now then
						retry[run] = nil
					end
				end
				model.retry = retry
				for _, repo in ipairs(heiko.await(cmd.repos())) do
					wake_due(heiko.store.of(repo), now, model)
				end
			end,
		},
	},

	-- What the chat has waiting, in its inspector, with a way to cancel
	-- each; and the messages it woke on, each wake-up's time a figure.
	view = function(state, env)
		local view = {}
		local list = state.waiting or {}
		if #list > 0 then
			-- Its section's head, with how many wait, over a ruled row each.
			local rows = { ui.header("Scheduled", { meta = #list }) }
			for index, one in ipairs(list) do
				if index > 1 then
					table.insert(rows, ui.rule())
				end
				table.insert(rows, row(env.run.id, one))
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
