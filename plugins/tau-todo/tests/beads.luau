local tau = require("tau")
local t = tau.test
local beads = require("beads")
local time = require("time")

t.case("times read back as they were written", function()
	for _, unix in ipairs({ 0, 951782400, 1793714400, 4102444799 }) do
		t.equal(time.parse(time.iso(unix)), unix)
	end
	t.equal(time.iso(1793714400), "2026-11-03T14:00:00Z")
end)

t.case("an export reads back as every field it had, and is the same bytes again", function()
	local issues = {
		["td-1"] = {
			id = "td-1",
			title = "fix the sync",
			description = "It drops a \"quote\".",
			criteria = { { text = "it works", done = true }, { text = "it is fast", done = false } },
			type = "bug",
			priority = 1,
			status = "blocked",
			blocked = "the vendor",
			labels = { "phone", "sync" },
			comments = { { at = 1793714400, by = "r1", text = "line one\nline two" } },
			links = { { id = "td-2", kind = "blocks" }, { id = "td-2", kind = "related" } },
			created = 1793710000,
			updated = 1793714400,
		},
		["td-2"] = {
			id = "td-2",
			title = "later",
			description = "",
			criteria = {},
			type = "task",
			priority = 3,
			status = "closed",
			labels = {},
			comments = {},
			links = {},
			created = 1793700000,
			updated = 1793700500,
			closed = 1793700500,
			close_reason = "done",
			defer_until = 1793800000,
			assignee = "alice",
		},
	}
	local text = beads.export(issues)
	t.equal(beads.import(text), issues)
	t.equal(beads.export(beads.import(text)), text)
	t.equal(string.sub(text, 1, 18), '{"id":"td-1","titl')
end)

-- A Beads assignee is a person, not a tau run: it is kept as the issue's
-- assignee, and claims nothing, so an open or closed issue it names is
-- not held, and a run can claim it. It made a claim whatever the status,
-- which nothing let go.
t.case("an assignee in a Beads file claims nothing", function()
	local text = '{"id":"td-7","title":"pick me","status":"open","priority":2,"issue_type":"task",'
		.. '"assignee":"alice","created_at":"2026-11-03T14:00:00Z","updated_at":"2026-11-03T14:00:00Z"}\n'
	local issue = beads.import(text)["td-7"]
	t.equal(issue.claim, nil)
	t.equal(issue.assignee, "alice")
	t.equal(issue.status, "open")
	t.equal(beads.export(beads.import(text)), beads.export({ ["td-7"] = issue }))
end)
