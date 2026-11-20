-- heiko-skills in a run: the list in the instructions, the tool loading a
-- skill and its step; and outside one, the page, the sidebar's count and
-- the composer's commands.
local heiko = require("heiko")
local t = heiko.test

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
	local text, loaded = r:tool("skill", { name = "release-notes" }, "c1")
	t.equal(
		text,
		"Skill release-notes. Its folder, where the files it names are: /mine/release-notes\n\nGroup the commits by kind."
	)
	local ok, why = pcall(r.tool, r, "skill", { name = "nope" })
	t.equal(ok, false)
	t.equal(why, 'There is no skill "nope". The skills are: principle-small, release-notes.')
	t.equal(r:step("skill", { name = "release-notes" }, loaded), {
		subject = "release-notes",
		receipt = { heiko.ui.receipt.count(1, "line") },
		body = heiko.ui.form.code("markdown", "Group the commits by kind."),
	})
end)

t.case("the page shows each skill, and the folder's button reveals it", function()
	local r = run()
	local app = r:app()
	t.equal(app.nav, { detail = "2 skills" })
	local page = r:app_page(app)
	local text = json.encode(page)
	t.equal(string.find(text, "1 file · built in", 1, true) ~= nil, true, text)
	t.equal(string.find(text, "2 files", 1, true) ~= nil, true, text)
	t.equal(string.find(text, '"action":"heiko.reveal"', 1, true) ~= nil, true, text)
end)

t.case("each skill but the principles is a command that sends its name", function()
	local r = run()
	local listed = r:commands(r:app())
	t.equal(listed["release-notes"], { description = "Writes release notes", args = "what to do", icon = "skill" })
	t.equal(listed["principle-small"], nil)
	t.equal(r:action("/release-notes", { text = "since v1" }), heiko.interface("send", { text = "/release-notes since v1" }))
	t.equal(r:action("/release-notes", { text = "" }), heiko.interface("send", { text = "/release-notes" }))
end)
