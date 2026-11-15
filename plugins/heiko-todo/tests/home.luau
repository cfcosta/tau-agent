-- What Home lists of a repository's issues: the first ready ones, as
-- `repo_data`'s `home`. The fixture `demo` is what heiko's demo
-- (crates/heiko-demo) shows there of heiko-agent.
local heiko = require("heiko")
local t = heiko.test

-- A run whose repository has the demo's issues: three ready, and one
-- that waits on the first.
local function demo_run()
	local run = t.run({ id = "r1", now = "2026-11-05T09:00:00Z" })
	local log = run:tool("todo_create", { title = "Log malformed headers once per run", priority = "P3" }).issue.id
	run:at("2026-11-05T09:10:00Z")
	run:tool("todo_create", { title = "Jitter on top of the hint", priority = "P2" })
	run:at("2026-11-05T09:20:00Z")
	run:tool("todo_create", { title = "Retry budget per request", priority = "P2" })
	run:at("2026-11-05T09:30:00Z")
	local docs = run:tool("todo_create", { title = "Document the header's log line", priority = "P1" }).issue.id
	run:tool("todo_link", { id = docs, kind = "blocks", on = log })
	return run
end

-- A list's rows, as plain tables.
local function plain(rows)
	local out = {}
	for _, row in ipairs(rows) do
		table.insert(out, { id = row.id, title = row.title, detail = row.detail, write = row.write })
	end
	return out
end

t.case("home lists the ready issues, each writing what a run on it starts with", function()
	local run = t.run({ id = "r1", now = "2026-11-05T09:00:00Z" })
	t.equal(#run:repo_data().home.rows, 0)
	local a = run:tool("todo_create", { title = "fix the sync", priority = "P1" }).issue.id
	run:at("2026-11-05T10:00:00Z")
	local b = run:tool("todo_create", { title = "draw the board", priority = "P0" }).issue.id
	local c = run:tool("todo_create", { title = "write the docs" }).issue.id
	run:tool("todo_link", { id = c, kind = "blocks", on = a })
	local home = run:repo_data().home
	t.equal(home.title, "Ready to pick up")
	t.equal(home.meta, "this repository's issues")
	t.equal(plain(home.rows), {
		{ id = b, title = "draw the board", detail = "P0", write = "Work on " .. b .. ": draw the board." },
		{ id = a, title = "fix the sync", detail = "P1", write = "Work on " .. a .. ": fix the sync." },
	})
end)

t.case("home lists three issues at most", function()
	local run = t.run({})
	for n = 1, 5 do
		run:tool("todo_create", { title = "issue " .. n })
	end
	local data = run:repo_data()
	t.equal(data.ready, 5)
	t.equal(#data.home.rows, 3)
end)

t.case("the demo's home has three issues, by priority then age", function()
	local rows = demo_run():repo_data().home.rows
	local titles = {}
	for _, row in ipairs(rows) do
		table.insert(titles, row.title .. " " .. row.detail)
	end
	t.equal(titles, {
		"Jitter on top of the hint P2",
		"Retry budget per request P2",
		"Log malformed headers once per run P3",
	})
end)

t.fixture("demo", function()
	return { repo_data = { ["heiko-agent"] = demo_run():repo_data() } }
end)
