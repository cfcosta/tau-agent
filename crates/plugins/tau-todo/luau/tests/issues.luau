local tau = require("tau")
local t = tau.test

local function ids(list)
	local out = {}
	for _, issue in ipairs(list) do
		table.insert(out, issue.id)
	end
	return out
end

t.case("files issues, and what is ready is open, unblocked and sorted", function()
	local run = t.run({ id = "r1", now = "2026-11-05T09:00:00Z" })
	local a = run:tool("todo_create", { title = "fix the sync", priority = "P1" }).issue.id
	-- Younger, but more pressing: it comes first.
	run:at("2026-11-05T10:00:00Z")
	local b = run:tool("todo_create", { title = "draw the board", priority = "P0" }).issue.id
	local c = run:tool("todo_create", { title = "write the docs" }).issue.id
	run:tool("todo_link", { id = c, kind = "blocks", on = a })
	t.equal(ids(run:tool("todo_ready", {}).ready), { b, a })
	run:tool("todo_close", { id = a, reason = "fixed" })
	t.equal(ids(run:tool("todo_ready", {}).ready), { b, c })
end)

t.case("refuses a blocks link that closes a cycle, naming it", function()
	local run = t.run({})
	local a = run:tool("todo_create", { title = "a" }).issue.id
	local b = run:tool("todo_create", { title = "b" }).issue.id
	local c = run:tool("todo_create", { title = "c" }).issue.id
	run:tool("todo_link", { id = b, kind = "blocks", on = a })
	run:tool("todo_link", { id = c, kind = "blocks", on = b })
	local ok, why = pcall(function()
		run:tool("todo_link", { id = a, kind = "blocks", on = c })
	end)
	t.equal(ok, false)
	t.equal(string.find(tostring(why), a .. " blocks " .. b .. " blocks " .. c, 1, true) ~= nil, true, tostring(why))
end)

t.case("a claim is one run's, and ends with it, saying what it left", function()
	local shared = {}
	local one = t.run({ id = "r1", store = shared })
	local two = t.run({ id = "r2", store = shared })
	local id = one:tool("todo_create", { title = "fix it", criteria = { "it works" } }).issue.id
	one:tool("todo_claim", { id = id })
	local ok, why = pcall(function()
		two:tool("todo_claim", { id = id })
	end)
	t.equal(ok, false)
	t.equal(string.find(tostring(why), "r1", 1, true) ~= nil, true, tostring(why))
	one:tool("todo_steps", { steps = { { text = "read", status = "done" }, { text = "patch", status = "in_progress" } } })
	one:run_end({ stop = "Done", text = "half way" })
	local shown = two:tool("todo_show", { id = id }).issue
	t.equal(shown.status, "open")
	t.equal(shown.claim, nil)
	local note = shown.comments[1].text
	t.equal(string.find(note, "half way", 1, true) ~= nil, true, note)
	t.equal(string.find(note, "- patch", 1, true) ~= nil, true, note)
	two:tool("todo_claim", { id = id })
end)

t.case("a run that closed its issue leaves it closed", function()
	local run = t.run({ id = "r1" })
	local id = run:tool("todo_create", { title = "fix it" }).issue.id
	run:tool("todo_claim", { id = id })
	run:tool("todo_close", { id = id, reason = "fixed" })
	run:run_end({ stop = "Done", text = "done" })
	t.equal(run:tool("todo_show", { id = id }).issue.status, "closed")
end)

t.case("as tau starts, the claims of the runs it cut off end", function()
	local shared = {}
	local dead = t.run({ id = "dead", store = shared })
	local live = t.run({ id = "live", store = shared })
	local a = dead:tool("todo_create", { title = "a" }).issue.id
	local b = live:tool("todo_create", { title = "b" }).issue.id
	dead:tool("todo_claim", { id = a })
	live:tool("todo_claim", { id = b })
	live:recover({ runs = { "dead" } })
	t.equal(live:tool("todo_show", { id = a }).issue.status, "open")
	t.equal(live:tool("todo_show", { id = b }).issue.claim, "live")
end)

t.case("a tool given bad arguments changes nothing", function()
	local run = t.run({})
	local id = run:tool("todo_create", { title = "a" }).issue.id
	local before = t.run({ store = run.store }):tool("todo_show", { id = id }).issue
	local ok = pcall(function()
		run:tool("todo_update", { id = id, title = "b", priority = "P9" })
	end)
	t.equal(ok, false)
	t.equal(run:tool("todo_show", { id = id }).issue, before)
end)

t.case("an epic closes once its children have, and counts them", function()
	local run = t.run({})
	local epic = run:tool("todo_create", { title = "the board", type = "epic" }).issue.id
	local child = run:tool("todo_create", { title = "columns", parent = epic }).issue.id
	t.equal(child, epic .. ".1")
	t.equal(run:tool("todo_create", { title = "drag", parent = epic }).issue.id, epic .. ".2")
	local ok = pcall(function()
		run:tool("todo_close", { id = epic, reason = "done" })
	end)
	t.equal(ok, false)
	run:tool("todo_close", { id = child, reason = "done" })
	run:tool("todo_close", { id = epic .. ".2", reason = "done" })
	t.equal(run:tool("todo_show", { id = epic }).issue.progress, "2/2")
	run:tool("todo_close", { id = epic, reason = "done" })
end)

t.case("a deferred issue is ready again past its date", function()
	local run = t.run({ now = "2026-11-05T09:00:00Z" })
	local id = run:tool("todo_create", { title = "later" }).issue.id
	run:tool("todo_defer", { id = id, ["until"] = "2026-11-10" })
	t.equal(#run:tool("todo_ready", {}).ready, 0)
	run:at("2026-11-11T09:00:00Z")
	t.equal(ids(run:tool("todo_ready", {}).ready), { id })
end)

t.case("an issue filed while one is held is found while it, and a step becomes it", function()
	local run = t.run({ id = "r1" })
	local id = run:tool("todo_create", { title = "fix it" }).issue.id
	run:tool("todo_claim", { id = id })
	run:tool("todo_steps", { steps = { { text = "patch", status = "in_progress" }, { text = "refactor the store", status = "pending" } } })
	local found = run:tool("todo_create", { title = "refactor the store", step = 2 }).issue.id
	t.equal(run:tool("todo_show", { id = found }).issue.links, { { kind = "found_while", id = id } })
	t.equal(run.state.steps[2], { text = "refactor the store", status = "done", issue = found })
end)

t.case("dispatch asks the person, and once allowed starts a chat that holds the issue", function()
	local run = t.run({ id = "r1" })
	local id = run:tool("todo_create", { title = "fix it" }).issue.id
	local asked = run:tool("todo_dispatch", { id = id }).tau_allow
	t.equal(asked.action, "dispatch")
	local start = run:action("dispatch", asked.args).tau_chat
	t.equal(start["then"], "started")
	t.equal(run:tool("todo_show", { id = id }).issue.claim, "starting")
	run:action("started", { issue = id, run = "r-new", repo = "test" })
	t.equal(run:tool("todo_show", { id = id }).issue.claim, "r-new")
end)

t.case("a dispatch for an issue another run took meanwhile is refused, naming it", function()
	local shared = {}
	local run = t.run({ id = "r1", store = shared })
	local id = run:tool("todo_create", { title = "fix it" }).issue.id
	local asked = run:tool("todo_dispatch", { id = id }).tau_allow
	t.run({ id = "r2", store = shared }):tool("todo_claim", { id = id })
	local said = run:action("dispatch", asked.args)
	t.equal(type(said), "string")
	t.equal(string.find(said, "r2", 1, true) ~= nil, true, said)
end)

t.case("the band shows the issue held and the steps; the row its id", function()
	local run = t.run({ id = "r1" })
	local id = run:tool("todo_create", { title = "fix it", criteria = { "it works", "it is fast" } }).issue.id
	run:tool("todo_claim", { id = id })
	run:tool("todo_check", { id = id, criterion = 1 })
	local view = run:view()
	t.equal(view.band.title, id .. " · fix it, 1/2")
	t.equal(view.row.line, id)
end)

t.case("the board's columns hold what the model says, and a move is its action", function()
	local run = t.run({ id = "r1" })
	local a = run:tool("todo_create", { title = "a" }).issue.id
	local b = run:tool("todo_create", { title = "b" }).issue.id
	run:tool("todo_link", { id = b, kind = "blocks", on = a })
	local page = run:page("board", {})
	local board
	for _, piece in ipairs(page.children) do
		if piece.piece == "board" then
			board = piece
		end
	end
	local cards = {}
	for _, column in ipairs(board.columns) do
		cards[column.id] = {}
		for _, card in ipairs(column.cards) do
			table.insert(cards[column.id], card.id)
		end
	end
	t.equal(cards.ready, { a })
	t.equal(cards.blocked, { b })
	-- To In progress starts a chat on it.
	t.equal(run:action("move", { card = a, from = "ready", to = "in_progress" }).tau_chat["then"], "started")
	-- To Done asks why, then closes with the reason.
	run:action("move", { card = b, from = "blocked", to = "done" })
	run:action("close", { text = "not needed" })
	t.equal(run:tool("todo_show", { id = b }).issue.close_reason, "not needed")
	t.equal(run:repo_data().ready, 0)
end)
