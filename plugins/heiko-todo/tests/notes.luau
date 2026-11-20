-- The calls about an issue a run files are steps in its chat (ADR
-- 0060), each with the issue's id, the last with the undo.
local heiko = require("heiko")
local t = heiko.test

-- What a call folded away draws: no line, and no body.
local function away(call)
	t.equal(call.line, false)
	t.equal(call.body, nil)
end

-- A step's receipt, and its undo when it has one.
local function step(call)
	t.equal(call.line, nil)
	t.equal(call.body, nil)
	local undo = call.action
	if undo == false then
		undo = nil
	end
	return call.receipt[1].text, undo
end

t.case("an issue filed and claimed is a step a call with its id, the undo on the last; its steps are not there", function()
	local run = t.run({ id = "r1" })
	local id = run:tool("todo_create", { title = "Identify startup improvements" }, "c1").issue.id
	local label, undo = step(run:view().calls.c1)
	t.equal(label, id)
	t.equal(undo.label, "undo")
	t.equal(undo.args, { forget = id })

	run:tool("todo_claim", { id = id }, "c2")
	local calls = run:view().calls
	label, undo = step(calls.c1)
	t.equal({ label, undo }, { id, nil })
	label, undo = step(calls.c2)
	t.equal(label, id)
	t.equal(undo.args, { forget = id })

	run:tool("todo_steps", {
		steps = {
			{ text = "Profile startup", status = "done" },
			{ text = "Check launch overhead", status = "in_progress" },
			{ text = "Rank options", status = "pending" },
		},
	}, "c3")
	calls = run:view().calls
	t.equal(step(calls.c2), id)
	away(calls.c3)
end)

t.case("steps are one line over the composer while any is left", function()
	local run = t.run({ id = "r1" })
	run:tool("todo_steps", {
		steps = {
			{ text = "Profile startup", status = "done" },
			{ text = "Check launch overhead", status = "in_progress" },
			{ text = "Rank options", status = "pending" },
		},
	}, "c1")
	local view = run:view()
	away(view.calls.c1)
	local band = view.band
	t.equal({ band.piece, band.small, band.open, band.title }, { "section", true, false, "" })
	t.equal(band.lead[1], heiko.ui.pips({ "good", "warn", "neutral" }))
	t.equal({ band.lead[2].text, band.lead[3].text }, { "2 of 3", "Check launch overhead" })
	t.equal(#band.children, 3)
end)

t.case("steps all done leave the composer, and one line where the last call is", function()
	local run = t.run({ id = "r1" })
	local done = { { text = "read", status = "done" }, { text = "patch", status = "done" } }
	run:tool("todo_steps", { steps = { { text = "read", status = "done" }, { text = "patch", status = "in_progress" } } }, "c1")
	run:tool("todo_steps", { steps = done }, "c2")
	local view = run:view()
	t.equal(view.band, nil)
	away(view.calls.c1)
	local line = view.calls.c2.body
	t.equal(view.calls.c2.line, false)
	t.equal({ line.piece, line.small, line.open }, { "section", true, false })
	t.equal(line.lead[1].text, "2 steps")
	t.equal(line.lead[2], heiko.ui.pips({ "good", "good" }))
	t.equal(#line.children, 2)
	-- Sent again, the line moves to the later call.
	run:tool("todo_steps", { steps = done }, "c3")
	view = run:view()
	away(view.calls.c2)
	t.equal(view.calls.c3.body.lead[1].text, "2 steps")
end)

t.case("closing the issue moves the undo to its step, and the undo reopens it", function()
	local run = t.run({ id = "r1" })
	local id = run:tool("todo_create", { title = "fix it" }, "c1").issue.id
	run:tool("todo_claim", { id = id }, "c2")
	run:tool("todo_close", { id = id, reason = "fixed" }, "c3")
	local calls = run:view().calls
	t.equal({ step(calls.c1) }, { id })
	t.equal({ step(calls.c2) }, { id })
	local label, undo = step(calls.c3)
	t.equal(label, id)
	t.equal(undo.args.restore.status, "in_progress")
	run:action("undo", undo.args)
	label, undo = step(run:view().calls.c3)
	t.equal({ label, undo }, { id .. " · undone", nil })
	t.equal(run:tool("todo_show", { id = id }).issue.status ~= "closed", true)
end)

t.case("each issue filed has its own id and undo", function()
	local run = t.run({ id = "r1" })
	local a = run:tool("todo_create", { title = "a" }, "c1").issue.id
	local b = run:tool("todo_create", { title = "b" }, "c2").issue.id
	run:tool("todo_claim", { id = a }, "c3")
	local calls = run:view().calls
	t.equal({ step(calls.c1) }, { a })
	local label, undo = step(calls.c2)
	t.equal({ label, undo.args }, { b, { forget = b } })
	label, undo = step(calls.c3)
	t.equal({ label, undo.args }, { a, { forget = a } })
end)

t.case("calls about an issue the run did not file keep their lines", function()
	local shared = {}
	local id = t.run({ id = "r0", store = shared }):tool("todo_create", { title = "old" }, "c0").issue.id
	local run = t.run({ id = "r1", store = shared })
	run:tool("todo_claim", { id = id }, "c1")
	run:tool("todo_close", { id = id, reason = "done" }, "c3")
	t.equal(run:view().calls, nil)
end)

t.case("reads and other changes of a noted issue keep their own lines", function()
	local run = t.run({ id = "r1" })
	local id = run:tool("todo_create", { title = "fix it" }, "c1").issue.id
	run:tool("todo_show", { id = id }, "c2")
	run:tool("todo_comment", { id = id, text = "seen" }, "c3")
	run:tool("todo_update", { id = id, title = "fix it well" }, "c4")
	local calls = run:view().calls
	t.equal(calls.c2, nil)
	t.equal(calls.c3, nil)
	t.equal(calls.c4, nil)
	local label, undo = step(calls.c1)
	t.equal({ label, undo.args }, { id, { forget = id } })
end)

t.case("undoing the filing says so on its step, with nothing left to undo", function()
	local run = t.run({ id = "r1" })
	local id = run:tool("todo_create", { title = "fix it" }, "c1").issue.id
	run:action("undo", { forget = id })
	local label, undo = step(run:view().calls.c1)
	t.equal({ label, undo }, { id .. " · undone", nil })
	t.equal(#run:tool("todo_list", {}).issues, 0)
end)

-- The demo's chat: an issue filed and claimed, as two steps with its
-- id, and its steps over the composer.
t.fixture("note", function()
	local run = t.run({ id = "demo" })
	local id = run:tool("todo_create", { title = "Honor retry-after on 429 and 503" }, "call_todo_1").issue.id
	run:tool("todo_claim", { id = id }, "call_todo_2")
	run:tool("todo_steps", {
		steps = {
			{ text = "Find where the loop reads the header", status = "done" },
			{ text = "Pass the parsed header to the retry policy", status = "in_progress" },
			{ text = "Cap the wait at the policy's longest delay", status = "pending" },
			{ text = "Add tests against the fake server", status = "pending" },
		},
	}, "call_todo_3")
	return { state = run.state, view = run:view() }
end)
