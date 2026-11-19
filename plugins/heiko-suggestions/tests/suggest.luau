-- What a suggestion is asked from and what is kept: the conversation cut
-- to the person's last words, the calls and the last answer; the request
-- with its rules and what was turned down; a title cleaned to one short
-- line; the list, whose pending part an answer replaces; and the newest
-- pending one, which the composer says, for a week.
local heiko = require("heiko")
local t = heiko.test
local g = t.gen
local suggest = require("suggest")

local function said(role, text, extra)
	local message = extra or {}
	message.role, message.text = role, text
	return message
end

t.case("a conversation comes to the person's last words, the calls and the last answer", function()
	local digest = suggest.digest({
		said("user", "heiko's preamble, long\n\nwhy does the sync test fail?"),
		said("assistant", "", { calls = { { name = "vcs_status" }, { name = "exec_command" } } }),
		said("toolResult", "M src/sync.rs"),
		said("toolResult", "test sync_test ... FAILED"),
		said("assistant", "It races on the clock."),
	})
	t.equal(digest.asks, { "heiko's preamble, long\n\nwhy does the sync test fail?" })
	t.equal(digest.calls, {
		{ name = "vcs_status", result = "M src/sync.rs" },
		{ name = "exec_command", result = "test sync_test ... FAILED" },
	})
	t.equal(digest.answer, "It races on the clock.")
end)

t.case("a conversation where the person said nothing asks for nothing", function()
	t.equal(suggest.digest({}), nil)
	t.equal(suggest.digest({ said("user", "  "), said("assistant", "hi") }), nil)
end)

t.case("a long message keeps its end, where the person's words are", function()
	local long = string.rep("x", 1000) .. " fix the flaky test"
	local ask = suggest.digest({ said("user", long) }).asks[1]
	t.equal(utf8.len(ask), suggest.ASK_CHARS + 1)
	t.equal(string.sub(ask, 1, #"…"), "…")
	t.equal(string.sub(ask, -#"fix the flaky test"), "fix the flaky test")
end)

t.case("the request has its rules, what was turned down and the conversation", function()
	local request = suggest.request({
		asks = { "why does the sync test fail?" },
		calls = { { name = "exec_command", result = "FAILED" } },
		answer = "It races on the clock.",
	}, { "Rewrite the clock" }, 2)
	for _, part in ipairs({
		"Suggest at most 2 next runs",
		"Do not suggest deleting, publishing, pushing",
		"do not suggest them again:\n- Rewrite the clock",
		"<ask>\nwhy does the sync test fail?\n</ask>",
		"- exec_command: FAILED",
		"<answer>\nIt races on the clock.\n</answer>",
	}) do
		t.equal(string.find(request, part, 1, true) ~= nil, true, part)
	end
	t.equal(suggest.schema(2).properties.suggestions.maxItems, 2)
end)

t.case("a title is one short line, without its quotes or its last stop", function()
	t.equal(suggest.clean('  "Fix the   sync\ntest."  '), "Fix the sync test")
	t.equal(suggest.clean("“Retry it”?!"), "Retry it")
	t.equal(suggest.clean(" .. "), nil)
	-- A dash is not a quote: its bytes stay whole.
	t.equal(suggest.clean("—Ship—"), "—Ship—")
	t.equal(utf8.len(suggest.clean(string.rep("é", 80))), suggest.TITLE_CHARS)
end)

local function pending(id, at, title)
	return { id = id, title = title or id, prompt = "do " .. id, why = "", run = "r0", model = "m", at = at, status = "pending" }
end

t.case("an answer replaces the pending ones, and the turned down stay", function()
	local list = {
		pending("old", 10),
		{ id = "took", title = "Took it", prompt = "p", at = 5, status = "taken" },
		{ id = "no", title = "Not this", prompt = "p", at = 6, status = "dismissed" },
	}
	local merged, added = suggest.merge(list, {
		{ title = "Add a retry", prompt = "Add a retry to sync.", why = "it raced" },
		-- Turned down before: not again.
		{ title = "not this", prompt = "p", why = "" },
		-- Nothing left of it.
		{ title = "...", prompt = "p", why = "" },
		{ title = "Third", prompt = "  ", why = "" },
	}, "r1", "gpt-luna", 100, 3)
	t.equal(added, 1)
	t.equal(#merged, 3)
	t.equal(merged[1].id, "took")
	t.equal(merged[2].id, "no")
	t.equal(merged[3], {
		id = "100-1",
		title = "Add a retry",
		prompt = "Add a retry to sync.",
		why = "it raced",
		run = "r1",
		model = "gpt-luna",
		at = 100,
		status = "pending",
	})
end)

t.case("the composer says the newest pending one, for a week", function()
	local list = { pending("a", 100), pending("b", 200), { id = "c", title = "c", prompt = "p", at = 300, status = "taken" } }
	t.equal(suggest.hint(list, 1000), { placeholder = "b", prompt = "do b" })
	t.equal(suggest.hint(list, 200 + suggest.SHOWN_FOR), nil)
	t.equal(suggest.hint({}, 0), nil)
end)

t.case("a prompt or an id marks its pending suggestion", function()
	local list, marked = suggest.mark({ pending("a", 1), pending("b", 2) }, "  do b ", "taken")
	t.equal(marked, true)
	t.equal(list[2].status, "taken")
	list, marked = suggest.mark(list, "b", "dismissed")
	t.equal(marked, false, "taken already")
	list, marked = suggest.mark(list, "a", "dismissed")
	t.equal(marked, true)
	t.equal(suggest.turned_down(list), { "b", "a" })
end)

t.case("the model is the plan's newest of the family", function()
	local models = { { id = "gpt-6.2-sol", family = "sol" }, { id = "gpt-6-luna", family = "luna" } }
	t.equal(suggest.model_of(models, "luna"), "gpt-6-luna")
	t.equal(suggest.model_of({}, "luna"), "luna")
end)

local units = g.sampled_from({ "a", "Z", "é", "語", "🦀", " ", "\n", '"', "“", "”", ".", "?" })

local function text_of(draw, max)
	return table.concat(draw(g.lists(units, { max = max })))
end

-- Whatever the conversation, what is kept of it is bounded: the last few
-- asks, each at most its characters and an ellipsis, the last calls, and
-- whole characters throughout.
t.property("a digest is bounded", function(draw)
	local messages = {}
	for _ = 1, draw(g.integers({ min = 0, max = 12 })) do
		local role = draw(g.sampled_from({ "user", "assistant", "toolResult" }))
		local message = said(role, text_of(draw, 900))
		if role == "assistant" then
			message.calls = { { name = "exec_command" } }
		end
		table.insert(messages, message)
	end
	local digest = suggest.digest(messages)
	if digest == nil then
		return
	end
	t.equal(#digest.asks <= suggest.ASKS, true)
	t.equal(#digest.calls <= suggest.CALLS, true)
	for _, ask in ipairs(digest.asks) do
		t.equal(utf8.len(ask) ~= nil and utf8.len(ask) <= suggest.ASK_CHARS + 1, true, ask)
	end
	if digest.answer ~= nil then
		t.equal(utf8.len(digest.answer) <= suggest.ANSWER_CHARS + 1, true)
	end
end, { cases = 200 })

-- A cleaned title is whole characters, one line, at most TITLE_CHARS,
-- and neither starts with a quote nor ends with a stop.
t.property("a clean title is one short line", function(draw)
	local title = suggest.clean(text_of(draw, 90))
	if title == nil then
		return
	end
	t.equal(utf8.len(title) ~= nil, true, "whole characters")
	t.equal(utf8.len(title) <= suggest.TITLE_CHARS, true, title)
	t.equal(string.find(title, "\n", 1, true), nil, title)
	t.equal(heiko.text.trim(title), title)
	t.equal(string.sub(title, 1, 1) ~= '"', true, title)
end, { cases = 300 })
