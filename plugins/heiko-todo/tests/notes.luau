-- An issue a run files has one note in its chat: the calls about it
-- fold into it, drawn where the last of them is.
local heiko = require("heiko")
local t = heiko.test

-- What a call folded away draws: a head that is an empty stack, and no
-- body.
local function away(call)
	t.equal(call.head.piece, "stack")
	t.equal(#call.head.children, 0)
	t.equal(call.body, nil)
end

-- A note's parts: its mark, caption, id, undo and title.
local function parts(call)
	t.equal(#call.head.children, 0)
	local note = call.body
	t.equal(note.piece, "row")
	local mark, body = note.children[1], note.children[2].children
	local head = body[1].children
	t.equal(#body, 2)
	return {
		mark = { mark.piece, mark.text, mark.tone },
		caption = head[1].text,
		id = head[3].text,
		undo = head[4],
		slip = { body[2].piece, body[2].text, body[2].tone, body[2].dashed },
	}
end

t.case("an issue filed and claimed is one note, at the last call; its steps are not in it", function()
	local run = t.run({ id = "r1" })
	local id = run:tool("todo_create", { title = "Identify startup improvements" }, "c1").issue.id
	local filed = parts(run:view().calls.c1)
	t.equal(filed.mark, { "kanji", "件", "good" })
	t.equal(filed.caption, "filed")
	t.equal(filed.id, id)
	t.equal(filed.slip, { "slip", "Identify startup improvements", "good", false })
	t.equal(filed.undo.label, "undo")
	t.equal(filed.undo.args, { forget = id })

	run:tool("todo_claim", { id = id }, "c2")
	local calls = run:view().calls
	away(calls.c1)
	t.equal(parts(calls.c2).caption, "filed · claimed by this run")
	t.equal(parts(calls.c2).undo.args, { forget = id })

	run:tool("todo_steps", {
		steps = {
			{ text = "Profile startup", status = "done" },
			{ text = "Check launch overhead", status = "in_progress" },
			{ text = "Rank options", status = "pending" },
		},
	}, "c3")
	calls = run:view().calls
	away(calls.c1)
	t.equal(parts(calls.c2).caption, "filed · claimed by this run")
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
	t.equal(#view.calls.c2.head.children, 0)
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

t.case("closing the issue folds in too, and its undo reopens it", function()
	local run = t.run({ id = "r1" })
	local id = run:tool("todo_create", { title = "fix it" }, "c1").issue.id
	run:tool("todo_claim", { id = id }, "c2")
	run:tool("todo_close", { id = id, reason = "fixed" }, "c3")
	local calls = run:view().calls
	away(calls.c1)
	away(calls.c2)
	local note = parts(calls.c3)
	t.equal(note.caption, "filed · claimed by this run · closed")
	t.equal(note.undo.args.restore.status, "in_progress")
	run:action("undo", note.undo.args)
	note = parts(run:view().calls.c3)
	t.equal(note.caption, "filed · claimed by this run · closed · undone")
	t.equal(note.undo, nil)
	t.equal(run:tool("todo_show", { id = id }).issue.status ~= "closed", true)
end)

t.case("each issue filed has its own note", function()
	local run = t.run({ id = "r1" })
	local a = run:tool("todo_create", { title = "a" }, "c1").issue.id
	local b = run:tool("todo_create", { title = "b" }, "c2").issue.id
	run:tool("todo_claim", { id = a }, "c3")
	local calls = run:view().calls
	away(calls.c1)
	t.equal(parts(calls.c2).id, b)
	t.equal(parts(calls.c2).caption, "filed")
	t.equal(parts(calls.c3).id, a)
	t.equal(parts(calls.c3).caption, "filed · claimed by this run")
end)

t.case("calls about an issue the run did not file keep their lines", function()
	local shared = {}
	local id = t.run({ id = "r0", store = shared }):tool("todo_create", { title = "old" }, "c0").issue.id
	local run = t.run({ id = "r1", store = shared })
	run:tool("todo_claim", { id = id }, "c1")
	run:tool("todo_close", { id = id, reason = "done" }, "c3")
	t.equal(run:view().calls, nil)
end)

t.case("reads and other changes of a noted issue keep their lines; a new title shows on the slip", function()
	local run = t.run({ id = "r1" })
	local id = run:tool("todo_create", { title = "fix it" }, "c1").issue.id
	run:tool("todo_show", { id = id }, "c2")
	run:tool("todo_comment", { id = id, text = "seen" }, "c3")
	run:tool("todo_update", { id = id, title = "fix it well" }, "c4")
	local calls = run:view().calls
	t.equal(calls.c2, nil)
	t.equal(calls.c3, nil)
	t.equal(calls.c4, nil)
	t.equal(parts(calls.c1).slip[2], "fix it well")
end)

t.case("undoing the filing says so on the note, with nothing left to undo", function()
	local run = t.run({ id = "r1" })
	local id = run:tool("todo_create", { title = "fix it" }, "c1").issue.id
	run:action("undo", { forget = id })
	local note = parts(run:view().calls.c1)
	t.equal(note.caption, "filed · undone")
	t.equal(note.undo, nil)
	t.equal(#run:tool("todo_list", {}).issues, 0)
end)

-- The demo's chat: an issue filed and claimed, as the one note, and
-- its steps over the composer.
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
