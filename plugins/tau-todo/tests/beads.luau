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
			claim = "r9",
		},
	}
	local text = beads.export(issues)
	t.equal(beads.import(text), issues)
	t.equal(beads.export(beads.import(text)), text)
	t.equal(string.sub(text, 1, 18), '{"id":"td-1","titl')
end)
