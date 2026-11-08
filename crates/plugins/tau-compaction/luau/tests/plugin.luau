-- tau-compaction in a run: past the threshold the older messages are
-- summarized in a request of their own and replaced by the summary; a
-- second compaction updates the first; a split turn's start gets its
-- own summary; a failed summary waits before it is tried again; an
-- overflow compacts whatever the window, and fails with the summary's
-- error; an idle chat is summarized in its own conversation, when idle
-- compaction is on.
local tau = require("tau")
local t = tau.test
local compaction = require("compaction")

local SETTINGS = { idle = true, reserve_tokens = 100, keep_recent_tokens = 1, window = 1000 }

local function contains(text, part)
	return string.find(text, part, 1, true) ~= nil
end

local function starts_with(text, part)
	return string.sub(text, 1, #part) == part
end

local function ends_with(text, part)
	return string.sub(text, -#part) == part
end

local function user(text, tokens)
	return { role = "user", text = text, tokens = tokens or 1 }
end

local function read(id, path, tokens)
	return {
		role = "assistant",
		text = "",
		tokens = tokens or 1,
		calls = { { id = id, name = "read", args = { path = path }, order = { "path" }, json = '{"path":"' .. path .. '"}' } },
	}
end

local function result(id, text)
	return { role = "toolResult", text = text, tokens = 1, call = id, name = "read", ok = true }
end

-- `messages` as `rewrite_context` reads them, after `turn`, at `tokens`.
local function view(messages, spec)
	spec = spec or {}
	for at, message in ipairs(messages) do
		message.index = at
		message.calls = message.calls or {}
	end
	return {
		trigger = spec.trigger or "turn_end",
		turn = spec.turn or 1,
		tokens = spec.tokens or 5000,
		window = spec.window,
		messages = messages,
	}
end

-- A run whose model answers each summary with the next of `answers`,
-- keeping what it was asked in `asked`.
local function compacting(answers, spec)
	spec = spec or {}
	local asked = {}
	local run = t.run({
		settings = spec.settings or SETTINGS,
		state = spec.state,
		tools = {
			infer = function(args)
				table.insert(asked, args)
				local answer = answers[#asked]
				if type(answer) == "string" then
					return { ok = true, value = answer }
				end
				return answer or { ok = false, error = "no answer left" }
			end,
		},
	})
	return run, asked
end

local function first_run()
	return { user("go"), read("c1", "src/lib.rs"), result("c1", "contents of src/lib.rs"), user("and then this") }
end

t.case("past the threshold, the older messages become a summary of their own request", function()
	local run, asked = compacting({ "## Goal\nship it" })
	run:start({ input = "go", effort = "high" })
	local rewrite = run:rewrite_context(view(first_run(), { tokens = 5010 }))
	t.equal(#asked, 1)
	local summary = asked[1]
	t.equal(summary.instructions, compaction.SYSTEM_PROMPT)
	t.equal(summary.effort, "high")
	t.equal(summary.conversation, nil)
	t.equal(starts_with(summary.task, "<conversation>\n[User]: go"), true, summary.task)
	t.equal(contains(summary.task, 'read(path="src/lib.rs")'), true)
	t.equal(contains(summary.task, "and then this"), false)
	t.equal(ends_with(summary.task, compaction.SUMMARIZATION_PROMPT), true)
	-- The summary goes first, then the message kept.
	t.equal(rewrite.rewrite.keep, { 4 })
	t.equal(#rewrite.rewrite.insert, 1)
	local opening = rewrite.rewrite.insert[1]
	t.equal(opening.before, 4)
	t.equal(starts_with(opening.text, compaction.SUMMARY_PREFIX), true)
	t.equal(contains(opening.text, "## Goal\nship it"), true)
	t.equal(contains(opening.text, "<read-files>\nsrc/lib.rs\n</read-files>"), true)
	t.equal(rewrite.rewrite.drops_conversation, true)
	local record = run.state.compacted
	t.equal(record.tokens_before, 5010)
	t.equal(record.read_files, { "src/lib.rs" })
	t.equal(rewrite.rewrite.details, record)
	t.equal(run:view().status.text, "watching the window")
end)

t.case("under the threshold, or with no window, nothing is asked", function()
	local run, asked = compacting({ "s" })
	t.equal(run:rewrite_context(view(first_run(), { tokens = 900 })), nil)
	local open, nothing = compacting({ "s" }, { settings = { idle = true, reserve_tokens = 100, keep_recent_tokens = 1, window = 0 } })
	t.equal(open:rewrite_context(view(first_run(), { tokens = 10 ^ 9 })), nil)
	t.equal(#asked + #nothing, 0)
	-- The model's window counts when the settings name none.
	local known, by_model = compacting({ "s" }, { settings = { idle = true, reserve_tokens = 100, keep_recent_tokens = 1, window = 0 } })
	t.equal(known:rewrite_context(view(first_run(), { tokens = 950, window = 1000 })) ~= nil, true)
	t.equal(#by_model, 1)
end)

t.case("a second compaction updates the first summary and carries its files", function()
	local run, asked = compacting({ "first summary", "second summary" })
	local first = run:rewrite_context(view(first_run()))
	local opening = first.rewrite.insert[1].text
	local second = {
		user(opening),
		user("and then this"),
		read("c2", "b.rs"),
		result("c2", "contents of b.rs"),
		user("then finish"),
	}
	local rewrite = run:rewrite_context(view(second, { turn = 2 }))
	local update = asked[2].task
	t.equal(ends_with(update, compaction.UPDATE_SUMMARIZATION_PROMPT), true)
	t.equal(contains(update, "<previous-summary>\nfirst summary"), true)
	t.equal(contains(update, compaction.SUMMARY_PREFIX), false, "the summary is not summarized again")
	t.equal(starts_with(update, "<conversation>\n[User]: and then this"), true, update)
	local last = rewrite.rewrite.insert[1].text
	t.equal(contains(last, "second summary"), true)
	t.equal(contains(last, "<read-files>\nb.rs\nsrc/lib.rs\n</read-files>"), true, last)
end)

t.case("a transcript another rewrite replaced is summarized afresh", function()
	local run, asked = compacting({ "first summary", "fresh" })
	run:rewrite_context(view(first_run()))
	run:rewrite_context(view({ user("pruned"), user("next") }, { turn = 2 }))
	t.equal(ends_with(asked[2].task, compaction.SUMMARIZATION_PROMPT), true)
	t.equal(contains(asked[2].task, "[User]: pruned"), true)
end)

t.case("a split turn's start is summarized on its own and merged under the history", function()
	local run, asked = compacting({ "prefix summary" })
	local messages = { user("go"), read("c1", "a.rs", 3), result("c1", "a"), read("c2", "b.rs", 10), result("c2", "b") }
	local rewrite = run:rewrite_context(view(messages))
	-- Keeping 1 token keeps the last call and its result: the cut lands
	-- inside the only turn, whose history is empty.
	t.equal(#asked, 1)
	t.equal(ends_with(asked[1].task, compaction.TURN_PREFIX_SUMMARIZATION_PROMPT), true)
	t.equal(starts_with(asked[1].task, "# Conversation\n[User]: go"), true)
	t.equal(rewrite.rewrite.keep, { 4, 5 })
	t.equal(
		contains(rewrite.rewrite.insert[1].text, "No prior history.\n\n---\n\n**Turn Context (split turn):**\n\nprefix summary"),
		true
	)
	-- The files of what it summarized: the call kept is not.
	t.equal(contains(rewrite.rewrite.insert[1].text, "<read-files>\na.rs\n</read-files>"), true)
end)

t.case("a failed summary writes nothing, and is tried again after 2, then 4 turns", function()
	local failed = { ok = false, error = "called", reason = "tool_call" }
	local run, asked = compacting({ failed, failed, "## Goal\nship it" })
	t.equal(run:rewrite_context(view(first_run(), { turn = 1 })), nil)
	t.equal(run.logs, { "Summarization attempted to call a tool" })
	t.equal(run.state.compacted, nil)
	t.equal(run:rewrite_context(view(first_run(), { turn = 2 })), nil)
	t.equal(#asked, 1, "the next turn does not ask")
	t.equal(run:rewrite_context(view(first_run(), { turn = 3 })), nil)
	t.equal(#asked, 2, "two turns later it asks again")
	for turn = 4, 6 do
		run:rewrite_context(view(first_run(), { turn = turn }))
	end
	t.equal(#asked, 2, "and then four turns later")
	t.equal(run:rewrite_context(view(first_run(), { turn = 7 })) ~= nil, true)
	t.equal(run.state.failures, 0)
end)

t.case("an overflow compacts whatever the window, and fails with the summary's error", function()
	local run, asked = compacting({ "summary" }, { settings = { idle = true, reserve_tokens = 100, keep_recent_tokens = 1, window = 0 } })
	local rewrite = run:rewrite_context(view(first_run(), { trigger = "overflow", tokens = 3 }))
	t.equal(#asked, 1)
	t.equal(rewrite.rewrite.keep, { 4 })
	local failing = compacting({ { ok = false, error = "cut", reason = "length" } })
	local ok, why = pcall(function()
		return failing:rewrite_context(view(first_run(), { trigger = "overflow" }))
	end)
	t.equal(ok, false)
	t.equal(why, "Summarization failed: generation hit the token cap and the summary is incomplete")
	t.equal(failing.state.compacted, nil)
end)

t.case("an idle chat is summarized in its own conversation", function()
	local run, asked = compacting({ "## Goal\nidle summary", "## Goal\nupdated" })
	local messages = { user("go"), { role = "assistant", text = "first", tokens = 1 }, user("and then this"), {
		role = "assistant",
		text = "done",
		tokens = 1,
	} }
	local rewrite = run:rewrite_context(view(messages, { trigger = "idle" }))
	t.equal(#asked, 1)
	-- What it drops goes first, as it was sent: all but the reply kept.
	t.equal(asked[1].conversation, 3)
	t.equal(asked[1].instructions, nil)
	t.equal(asked[1].task, compaction.in_context_request(false))
	t.equal(rewrite.rewrite.keep, { 4 })
	t.equal(contains(rewrite.rewrite.insert[1].text, "idle summary"), true)
	-- The next one updates the summary the conversation opens with.
	local next = { user(rewrite.rewrite.insert[1].text), { role = "assistant", text = "done", tokens = 1 }, user("next"), {
		role = "assistant",
		text = "again",
		tokens = 1,
	} }
	run:rewrite_context(view(next, { trigger = "idle" }))
	t.equal(asked[2].task, compaction.in_context_request(true))
	t.equal(asked[2].conversation, 3)
end)

t.case("with idle compaction off, an idle chat is left as it is", function()
	local run, asked = compacting({ "s" }, { settings = { idle = false, reserve_tokens = 100, keep_recent_tokens = 1, window = 1000 } })
	t.equal(run:rewrite_context(view(first_run(), { trigger = "idle" })), nil)
	t.equal(#asked, 0)
end)

t.case("a cut that keeps everything asks nothing", function()
	local run, asked = compacting({ "s" }, { settings = { idle = true, reserve_tokens = 100, keep_recent_tokens = 10 ^ 6, window = 1000 } })
	t.equal(run:rewrite_context(view(first_run())), nil)
	t.equal(#asked, 0)
end)

t.case("its settings pane switches idle compaction", function()
	local run = compacting({})
	local pane = run:settings_view(SETTINGS)
	t.equal(pane.children[1].piece, "toggle")
	t.equal(pane.children[1].key, "idle")
	t.equal(pane.children[1].label, "Compact idle chats")
	t.equal(t.load().settings.default.idle, true)
end)
