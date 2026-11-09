-- Chats' wake-ups (ADR 0039) against a model, with tau closing in the
-- middle of waking chats: chats schedule and cancel, time passes, chats
-- get busy, idle or end, and the job wakes them, sometimes cut off after
-- the turn of one it woke started.
--
-- tau keeps what the job wrote to the store as each wake starts a turn,
-- so closing after the k-th wake started its turn leaves the store as
-- it was when that wake was asked for: the earlier chats' wake-ups gone,
-- that chat's still there. A wake-up wakes its chat at least once: never
-- missed, once when tau stays up, and at most once more for each time
-- tau closed after its turn started and before it kept that it went.
local tau = require("tau")
local t = tau.test
local g = t.gen
local wake = require("wake")

local CHATS = { "chat-a", "chat-b" }
local NAMES = { "recheck", "merge", "ping" }

-- 2026-10-12T08:00:00Z.
local START = 1791792000

-- `unix` as an ISO time, in UTC.
local function iso(unix)
	local days, seconds = unix // 86400, unix % 86400
	local z = days + 719468
	local era = z // 146097
	local doe = z - era * 146097
	local yoe = (doe - doe // 1460 + doe // 36524 - doe // 146096) // 365
	local doy = doe - (365 * yoe + yoe // 4 - yoe // 100)
	local mp = (5 * doy + 2) // 153
	local d = doy - (153 * mp + 2) // 5 + 1
	local m = mp < 10 and mp + 3 or mp - 9
	local y = yoe + era * 400 + (m <= 2 and 1 or 0)
	return string.format(
		"%04d-%02d-%02dT%02d:%02d:%02dZ",
		y,
		m,
		d,
		seconds // 3600,
		seconds % 3600 // 60,
		seconds % 60
	)
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

-- Each chat's run, and the host's, which runs the job: one store.
local function open()
	local m = {
		now = START,
		store = {},
		waiting = {},
		states = { ["chat-a"] = "idle", ["chat-b"] = "idle" },
		fired = {},
		again = {},
		next = 0,
		-- What a wake answers: a chat's state, unless a test says else.
		answer = nil,
	}
	local function answer(run, text)
		return m.answer(run, text)
	end
	m.runs = {}
	for _, chat in ipairs(CHATS) do
		m.runs[chat] = t.run({ id = chat, kind = "chat", now = iso(START), store = m.store })
	end
	m.host = t.run({ id = "host", now = iso(START), store = m.store, wake = answer })
	return m
end

-- The answer a chat gives as it is woken, counting the turn it starts.
local function woken(m, run, text)
	local state = m.states[run]
	if state == "idle" then
		for _, one in ipairs(wake.parse(text)) do
			m.fired[one.note] = (m.fired[one.note] or 0) + 1
		end
		return "started"
	end
	return state
end

-- The chats with something due now, in the order the job wakes them.
local function due_runs(m)
	local runs, seen = {}, {}
	for _, one in pairs(m.waiting) do
		if one.at <= m.now and not seen[one.chat] then
			seen[one.chat] = true
			table.insert(runs, one.chat)
		end
	end
	table.sort(runs)
	return runs
end

-- The model's chat `chat` as a wake leaves it: woken, its due ones go;
-- gone, all of it goes.
local function settle(m, chat)
	for key, one in pairs(m.waiting) do
		if one.chat == chat then
			if m.states[chat] == "gone" or (m.states[chat] == "idle" and one.at <= m.now) then
				m.waiting[key] = nil
			end
		end
	end
end

local function pick(d, list)
	return d(g.sampled_from(list))
end

t.machine("a wake-up wakes its chat at least once, and once more at most per close", {
	init = function()
		return open()
	end,
	rules = {
		-- A chat schedules a wake-up, maybe in place of one of the same
		-- name, a minute to three hours ahead.
		schedule = {
			weight = 3,
			run = function(m, d)
				local chat, name = pick(d, CHATS), pick(d, NAMES)
				local after = d(g.integers({ min = 60, max = 10800 }))
				m.next += 1
				local note = "note-" .. m.next
				m.runs[chat]:tool("schedule", { name = name, note = note, ["in"] = after .. "s" })
				m.waiting[chat .. "/" .. name] = { chat = chat, name = name, at = m.now + after, note = note }
			end,
		},
		cancel = function(m, d)
			local chat, name = pick(d, CHATS), pick(d, NAMES)
			m.runs[chat]:tool("unschedule", { name = name })
			m.waiting[chat .. "/" .. name] = nil
		end,
		time_passes = {
			weight = 2,
			run = function(m, d)
				m.now += d(g.integers({ min = 1, max = 7200 }))
				for _, run in pairs(m.runs) do
					run:at(iso(m.now))
				end
				m.host:at(iso(m.now))
			end,
		},
		a_chat_changes = function(m, d)
			m.states[pick(d, CHATS)] = pick(d, { "idle", "busy", "gone" })
		end,
		-- The job wakes what came due.
		tick = {
			weight = 3,
			run = function(m)
				m.answer = function(run, text)
					return woken(m, run, text)
				end
				m.host:job("wake")
				for _, chat in ipairs(due_runs(m)) do
					settle(m, chat)
				end
			end,
		},
		-- tau closes after the k-th wake of the job started its turn:
		-- what the job wrote up to that wake is kept, and nothing after.
		close_in_tick = {
			weight = 2,
			run = function(m, d)
				local k = d(g.integers({ min = 1, max = #CHATS }))
				local runs = due_runs(m)
				t.assume(#runs >= k)
				local calls, kept = 0, nil
				m.answer = function(run, text)
					calls += 1
					if calls == k then
						kept = copy(m.store)
					elseif calls > k then
						return "busy"
					end
					return woken(m, run, text)
				end
				m.host:job("wake")
				for key in pairs(m.store) do
					m.store[key] = nil
				end
				for key, value in pairs(kept) do
					m.store[key] = value
				end
				for at, chat in ipairs(runs) do
					if at < k then
						settle(m, chat)
					elseif at == k and m.states[chat] == "idle" then
						for _, one in pairs(m.waiting) do
							if one.chat == chat and one.at <= m.now then
								m.again[one.note] = (m.again[one.note] or 0) + 1
							end
						end
					end
				end
			end,
		},
	},
	invariant = function(m)
		for note, times in pairs(m.fired) do
			t.equal(times <= 1 + (m.again[note] or 0), true, note .. " woke its chat " .. times .. " times")
		end
		local kept = {}
		for _, item in pairs(m.store) do
			kept[item.run .. "/" .. item.name] = { chat = item.run, name = item.name, at = item.at, note = item.note }
		end
		t.equal(kept, m.waiting, "what chats have waiting")
	end,
}, { cases = 100 })
