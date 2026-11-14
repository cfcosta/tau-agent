-- tau-schedule in a chat and in its job: a wake-up is scheduled once, in
-- or on a time, a minute to a year ahead, and listed and cancelled; what
-- is asked wrongly comes back with why; a sub-agent schedules nothing;
-- the job wakes each chat on all that came due, keeps what waits on a
-- busy chat, drops what a gone chat had, and tries again later when a
-- turn did not start; each turn's start reads the store again; and the
-- inspector and the card show it all.
local tau = require("tau")
local t = tau.test
local wake = require("wake")

-- 2026-01-05 09:00 UTC, three hours east: 12:00 local.
local NOW = "2026-01-05T09:00:00Z"
local OFFSET = 3 * 3600

local function chat(spec)
	spec = spec or {}
	spec.id = spec.id or "r1"
	spec.now = spec.now or NOW
	spec.offset = spec.offset or OFFSET
	return t.run(spec)
end

-- Why `run:tool(name, args)` failed; it must fail.
local function refused(run, name, args)
	local ok, why = pcall(run.tool, run, name, args)
	t.equal(ok, false, "expected " .. name .. " to refuse")
	return why
end

t.case("a wake-up is scheduled in a while or on a local time, and listed", function()
	local run = chat()
	t.equal(
		run:tool("schedule", { name = "recheck", note = "Did #412 merge?", ["in"] = "2h" }),
		"Scheduled `recheck` for 2026-01-05 14:00. This chat wakes then with your note."
	)
	t.equal(
		run:tool("schedule", { name = "release", note = "Ship it.\nThen tell them.", on = "2026-01-06 09:30" }),
		"Scheduled `release` for 2026-01-06 09:30. This chat wakes then with your note."
	)
	t.equal(
		run:tool("scheduled", {}),
		"It is 2026-01-05 12:00. 2 of the 100 this chat may have:\n"
			.. "- recheck · 2026-01-05 14:00 · Did #412 merge?\n"
			.. "- release · 2026-01-06 09:30 · Ship it."
	)
	t.equal(run.store["wake/r1/release"].at, 1767681000)
	t.equal(
		run:tool("schedule", { name = "recheck", note = "Again.", ["in"] = "3d" }),
		"Replaced `recheck` for 2026-01-08 12:00. This chat wakes then with your note."
	)
	t.equal(run:tool("unschedule", { name = "release" }), "Cancelled `release`.")
	t.equal(run:tool("unschedule", { name = "release" }), "Nothing is scheduled as `release`.")
	t.equal(#run.state.waiting, 1)
	t.equal(run.state.waiting[1].note, "Again.")
end)

t.case("nothing scheduled says the time", function()
	t.equal(chat():tool("scheduled", {}), "It is 2026-01-05 12:00. Nothing is scheduled.")
end)

t.case("what is asked wrongly comes back with why", function()
	local run = chat()
	local function why(args)
		return refused(run, "schedule", args)
	end
	t.equal(
		why({ name = "Recheck", note = "x", ["in"] = "2h" }),
		"`Recheck` cannot name a wake-up: lower-case letters, digits, `-` and `_`, starting with a letter, at most 40"
	)
	t.equal(why({ name = "a", note = "  ", ["in"] = "2h" }), "Leave yourself a note: what to do then")
	t.equal(why({ name = "a", note = "x", every = "1d" }), wake.NO_REPEATS)
	t.equal(why({ name = "a", note = "x", days = { "Monday" } }), wake.NO_REPEATS)
	t.equal(why({ name = "a", note = "x", ["in"] = "soon" }), '`soon` is not a duration, like "2h"')
	t.equal(
		why({ name = "a", note = "x", on = "tomorrow" }),
		'`tomorrow` is not a local date and time, like "2026-01-06 12:00"'
	)
	t.equal(
		why({ name = "a", note = "x" }),
		'Say when with one of `in` ("2h") or `on` (a local date and time)'
	)
	t.equal(
		why({ name = "a", note = "x", ["in"] = "2h", on = "2026-01-06 09:30" }),
		'Say when with one of `in` ("2h") or `on` (a local date and time)'
	)
	t.equal(why({ name = "a", note = "x", ["in"] = "30s" }), "That is 2026-01-05 12:00: a minute from now is the soonest")
	t.equal(why({ name = "a", note = "x", ["in"] = "367d" }), "A year ahead is the furthest")
	t.equal(why({ name = "a", note = "x", ["in"] = "2h", repeat_ = true }), "unknown field `repeat_`")
	t.equal(run.store, {})
end)

t.case("a chat has at most a hundred waiting, though it may replace one", function()
	local store = {}
	for n = 1, wake.MAX_WAITING do
		store["wake/r1/w" .. n] = { run = "r1", name = "w" .. n, note = "x", at = 1767610000 + n, made = 0 }
	end
	local run = chat({ store = store })
	t.equal(
		refused(run, "schedule", { name = "more", note = "x", ["in"] = "2h" }),
		"This chat has 100 waiting, the most it may: cancel some first"
	)
	t.equal(
		run:tool("schedule", { name = "w1", note = "y", ["in"] = "2h" }),
		"Replaced `w1` for 2026-01-05 14:00. This chat wakes then with your note."
	)
end)

t.case("a sub-agent schedules nothing", function()
	local run = chat({ kind = "sub_agent" })
	t.equal(refused(run, "schedule", { name = "a", note = "x", ["in"] = "2h" }), wake.SUB_AGENTS)
	t.equal(run.store, {})
end)

-- A chat `run` that scheduled `name` to come due `after` seconds from
-- NOW, in `store`.
local function due_in(store, run, name, after, note)
	store["wake/" .. run .. "/" .. name] = {
		run = run,
		name = name,
		note = note or ("Note for " .. name),
		at = 1767603600 + after,
		made = 1767603600,
	}
end

t.case("the job wakes each chat on all that came due, in order", function()
	local store = {}
	due_in(store, "r1", "later", 120)
	due_in(store, "r1", "first", 60)
	due_in(store, "r1", "also", 60, "Line one\nLine two")
	due_in(store, "r2", "other", 3600)
	local run = chat({ store = store, stores = { elsewhere = {} } })
	run:at("2026-01-05T09:01:30Z")
	run:job("wake")
	t.equal(run.woken, {
		{
			run = "r1",
			text = "[scheduled · also · 2026-01-05 12:01]\nLine one\nLine two\n\n"
				.. "[scheduled · first · 2026-01-05 12:01]\nNote for first",
		},
	})
	t.equal(run.store["wake/r1/also"], nil)
	t.equal(run.store["wake/r1/first"], nil)
	t.equal(run.store["wake/r1/later"] ~= nil, true)
	t.equal(run.store["wake/r2/other"] ~= nil, true)
	-- Nothing more is due yet.
	run:job("wake")
	t.equal(#run.woken, 1)
end)

-- The turn a wake starts begins before the job lets go of what woke it:
-- its start lists what still waits, not what it woke on, though the
-- store keeps those until the job ends. One scheduled again for later
-- under the same name stays listed.
t.case("a woken turn does not list what woke it", function()
	local store = {}
	due_in(store, "r1", "first", 60)
	due_in(store, "r1", "later", 3600)
	local turn = chat({ store = store })
	turn:at("2026-01-05T09:01:30Z")
	local listed
	local host = chat({
		id = "host",
		store = store,
		stores = {},
		wake = function(run, text)
			turn:start({ input = text })
			listed = {}
			for _, one in ipairs(turn.state.waiting) do
				table.insert(listed, one.name)
			end
			return "started"
		end,
	})
	host:at("2026-01-05T09:01:30Z")
	host:job("wake")
	t.equal(listed, { "later" })
end)

t.case("the job wakes chats in every repository", function()
	local there = {}
	due_in(there, "r9", "deploy", 60)
	local run = chat({ stores = { other = there } })
	run:at("2026-01-05T09:01:00Z")
	run:job("wake")
	t.equal(run.woken, { { run = "r9", text = "[scheduled · deploy · 2026-01-05 12:01]\nNote for deploy" } })
	t.equal(there, {})
end)

t.case("a busy chat keeps what waits, and a gone one loses all of it", function()
	local store = {}
	due_in(store, "busy", "a", 60)
	due_in(store, "gone", "b", 60)
	due_in(store, "gone", "c", 86400)
	local answers = { busy = "busy", gone = "gone" }
	local run = chat({
		store = store,
		wake = function(r)
			return answers[r]
		end,
	})
	run:at("2026-01-05T09:02:00Z")
	run:job("wake")
	t.equal(#run.woken, 2)
	t.equal(run.store["wake/busy/a"] ~= nil, true)
	t.equal(run.store["wake/gone/b"], nil)
	t.equal(run.store["wake/gone/c"], nil)
	-- Once the busy chat's turn ends, it wakes.
	answers.busy = "started"
	run:job("wake")
	t.equal(run.woken[3].run, "busy")
	t.equal(run.store, {})
end)

t.case("a turn that did not start is tried again thirty seconds later", function()
	local store = {}
	due_in(store, "r1", "a", 60)
	local failing = true
	local run = chat({
		store = store,
		wake = function()
			if failing then
				error("the host is closing", 0)
			end
			return "started"
		end,
	})
	run:at("2026-01-05T09:01:00Z")
	run:job("wake")
	t.equal(#run.woken, 1)
	t.equal(run.store["wake/r1/a"] ~= nil, true)
	failing = false
	run:at("2026-01-05T09:01:29Z")
	run:job("wake")
	t.equal(#run.woken, 1)
	run:at("2026-01-05T09:01:30Z")
	run:job("wake")
	t.equal(#run.woken, 2)
	t.equal(run.store, {})
end)

t.case("one scheduled again while its chat woke stays", function()
	local store = {}
	due_in(store, "r1", "a", 60)
	local run
	run = chat({
		store = store,
		wake = function()
			-- The chat's turn schedules it again before the job goes on.
			due_in(run.store, "r1", "a", 7200, "Again")
			return "started"
		end,
	})
	run:at("2026-01-05T09:01:00Z")
	run:job("wake")
	t.equal(run.store["wake/r1/a"].note, "Again")
end)

t.case("each turn's start reads the store again, and keeps the card it woke on", function()
	local store = {}
	due_in(store, "r1", "later", 3600)
	local run = chat({ store = store, state = { waiting = { { name = "stale" } } } })
	local text = "[scheduled · first · 2026-01-05 12:01]\nNote for first"
	t.equal(run:start({ input = text }), nil)
	t.equal(#run.state.waiting, 1)
	t.equal(run.state.waiting[1].name, "later")
	t.equal(run.state.cards[text], { { name = "first", due = "2026-01-05 12:01", note = "Note for first" } })
	run:start({ input = "Just a message" })
	t.equal(run.state.cards["Just a message"], nil)
end)

t.case("the inspector lists what waits, each with a way to cancel it", function()
	local run = chat()
	run:tool("schedule", { name = "recheck", note = "Did #412 merge?", ["in"] = "2h" })
	local view = run:view()
	local rows = view.inspector.children
	t.equal(rows[1], { piece = "heading", text = "Scheduled" })
	local head = rows[2].children[1].children
	t.equal(head[2], { piece = "text", text = "recheck" })
	t.equal(head[4], { piece = "due", id = "r1-recheck", at = 1767610800000 })
	t.equal(head[5].action, "cancel")
	t.equal(head[5].icon, "close")
	run:action("cancel", head[5].args)
	t.equal(run.store, {})
	t.equal(run:view().inspector, nil)
	local ok = pcall(run.action, run, "cancel", { name = "recheck" })
	t.equal(ok, false)
end)

t.case("the message a chat woke on is a card", function()
	local run = chat()
	local text = "[scheduled · a · 2026-01-05 12:01]\nOne\n\n[scheduled · b · 2026-01-05 12:02]\nTwo"
	run:start({ input = text })
	local card = run:view().messages[text]
	t.equal(card.piece, "card")
	local blocks = card.children[1].children
	t.equal(#blocks, 3)
	t.equal(blocks[2], { piece = "rule" })
	t.equal(blocks[1].children[1].children[2], { piece = "text", text = "Scheduled · a" })
	t.equal(blocks[3].children[2], { piece = "text", text = "Two" })
end)
