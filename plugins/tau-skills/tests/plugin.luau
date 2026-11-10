-- tau-skills in a run: the list in the instructions, the tool loading a
-- skill and its card; and outside one, the page, the sidebar's count and
-- the composer's commands.
local tau = require("tau")
local t = tau.test

local FILES = {
	["/mine/release-notes/SKILL.md"] = {
		text = "---\nname: release-notes\ndescription: Writes release notes\n---\n\n  Group the commits by kind.\n",
	},
	["/mine/release-notes/scripts/tags.sh"] = "git tag",
	["/shipped/principle-small/SKILL.md"] = "---\nname: principle-small\ndescription: Keep it small\n---\nSmall.\n",
}
local ROOTS = { skills = "/mine", ["package/skills"] = "/shipped" }

local function run()
	return t.run({ files = FILES, roots = ROOTS })
end

t.case("a run lists the skills and loads one by name", function()
	local r = run()
	local plan = r:start({})
	t.equal(string.find(plan.instructions, "- release-notes: Writes release notes", 1, true) ~= nil, true)
	t.equal(string.find(plan.instructions, "## Principles", 1, true) ~= nil, true)
	t.equal(
		r:tool("skill", { name = "release-notes" }, "c1"),
		"Skill release-notes. Its folder, where the files it names are: /mine/release-notes\n\nGroup the commits by kind."
	)
	local ok, why = pcall(r.tool, r, "skill", { name = "nope" })
	t.equal(ok, false)
	t.equal(why, 'There is no skill "nope". The skills are: principle-small, release-notes.')
	local card = r:view().calls.c1
	t.equal(card.head.children[2].text, "release-notes")
	t.equal(card.body.children[1].text, "/mine/release-notes/SKILL.md")
end)

t.case("the page shows each skill, and the folder's button reveals it", function()
	local r = run()
	local app = r:app()
	t.equal(app.nav, { detail = "2 skills" })
	local page = r:app_page(app)
	local text = json.encode(page)
	t.equal(string.find(text, "1 file · built in", 1, true) ~= nil, true, text)
	t.equal(string.find(text, "2 files", 1, true) ~= nil, true, text)
	t.equal(string.find(text, '"action":"tau.reveal"', 1, true) ~= nil, true, text)
end)

t.case("each skill but the principles is a command that sends its name", function()
	local r = run()
	local listed = r:commands(r:app())
	t.equal(listed["release-notes"], { description = "Writes release notes", args = "what to do", icon = "skill" })
	t.equal(listed["principle-small"], nil)
	t.equal(r:action("/release-notes", { text = "since v1" }), tau.interface("send", { text = "/release-notes since v1" }))
	t.equal(r:action("/release-notes", { text = "" }), tau.interface("send", { text = "/release-notes" }))
end)
