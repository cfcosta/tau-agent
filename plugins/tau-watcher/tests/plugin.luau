-- The watcher in a run (docs/reference/watcher.md): the 6th step asks
-- once, `learn: none` keeps nothing, a note is kept and drawn, a reply it
-- cannot read is dropped, no second check while a note waits, and the
-- person's answers.
local tau = require("tau")
local t = tau.test
local watch = require("watch")

local ON = { enabled = true, family = "chat" }

local REPLY = "learn: The agent ran the suite in debug, rebuilding everything.\ntag: Heads up"

local TRANSCRIPT = {
	{ index = 1, role = "user", text = "run the tests" },
	{ index = 2, role = "assistant", text = "", calls = { { id = "c1", name = "exec_command", args = { cmd = "cargo test" } } } },
	{ index = 3, role = "toolResult", text = "ok", call = "c1", name = "exec_command", ok = true },
}

-- A run with the watcher on, the model answering `reply`, and every ask
-- kept in `asked`.
local function watching(reply, spec)
	spec = spec or {}
	local asked = {}
	local run = t.run({
		id = spec.id or "r1",
		kind = spec.kind,
		settings = spec.settings or ON,
		state = spec.state,
		transcript = TRANSCRIPT,
		tools = {
			infer = function(args)
				table.insert(asked, args)
				if reply == nil then
					return { ok = false, error = "the model is away" }
				end
				return { ok = true, value = reply }
			end,
		},
	})
	return run, asked
end

t.case("off by default, it asks nothing", function()
	local run, asked = watching(REPLY, { settings = { enabled = false, family = "chat" } })
	run:before_request({ replies = 5 })
	t.equal(#asked, 0)
end)

t.case("a sub-agent's run is never watched", function()
	local run, asked = watching(REPLY, { kind = "sub_agent" })
	run:before_request({ replies = 5 })
	t.equal(#asked, 0)
end)

t.case("the sixth step asks once, and none keeps nothing", function()
	local run, asked = watching("learn: none")
	for replies = 0, 12 do
		run:before_request({ replies = replies })
	end
	t.equal(#asked, 2, "at the 6th and the 12th step")
	t.equal(#watch.state(run.state).notes, 0)
	t.equal(string.find(asked[1].task, "[Agent ran]: exec_command(cmd=\"cargo test\")", 1, true) ~= nil, true, asked[1].task)
	t.equal(asked[1].model, nil, "the chat's own model")
end)

t.case("a note is kept, drawn under its step and above the composer", function()
	local run, asked = watching(REPLY)
	run:before_request({ replies = 5 })
	local state = watch.state(run.state)
	t.equal(#state.notes, 1)
	t.equal(state.notes[1].step, 6)
	t.equal(state.notes[1].tag, "Heads up")
	local view = run:view()
	t.equal(view.notes.n0 ~= nil, true, "under its step")
	t.equal(view.band ~= nil, true, "above the composer")
	t.equal(view.status.text, "1 note")
	-- It notes once a run.
	run:before_request({ replies = 11 })
	t.equal(#asked, 1)
end)

t.case("a reply it cannot read is dropped, and shows nothing", function()
	local run = watching("Sure! Here is a note.")
	run:before_request({ replies = 5 })
	local state = watch.state(run.state)
	t.equal(#state.notes, 0)
	t.equal(state.dropped, 1)
	t.equal(state.last_check, 6)
	t.equal(run:view().band, nil)
	local model_away = watching(nil)
	model_away:before_request({ replies = 5 })
	t.equal(watch.state(model_away.state).dropped, 1)
end)

t.case("no second check while a note waits; an answer lets it come, and lists the note", function()
	local first, asked = watching(REPLY)
	first:before_request({ replies = 5 })
	-- The next run of the chat: the note still waits.
	local next, again = watching(REPLY, { id = "r2", state = first.state })
	next:start({ input = "go on" })
	next:before_request({ replies = 11 })
	t.equal(#again, 0, "held back while the note waits")
	t.equal(watch.state(next.state).notes[1].typed_past, 1, "written past once")
	next:action("answer", { key = "n0", answer = "knew", line = watch.state(next.state).notes[1].line })
	next:before_request({ replies = 17 })
	t.equal(#again, 1, "asks again once answered")
	t.equal(string.find(again[1].task, "<person_already_knew>\n- The agent ran the suite", 1, true) ~= nil, true, again[1].task)
	t.equal(#asked, 1)
end)

t.case("chatting about a note puts it in the composer", function()
	local run = watching(REPLY)
	run:before_request({ replies = 5 })
	local line = watch.state(run.state).notes[1].line
	local answer = run:action("answer", { key = "n0", answer = "chatted", line = line })
	t.equal(answer, tau.interface("composer", { text = watch.chat_text(line) }))
	t.equal(watch.state(run.state).notes[1].status, "chatted")
	t.equal(run:view().band, nil, "answered, it leaves the band")
end)

t.case("the person's pick of family is the model asked", function()
	local run, asked = watching("learn: none", { settings = { enabled = true, family = "luna" } })
	run:before_request({ replies = 5 })
	t.equal(asked[1].model, "luna")
end)
