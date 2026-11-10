-- Reading skills folders: what makes a folder a skill, and why one is
-- not; the person's skills over those tau ships; and the list a run's
-- instructions get.
local tau = require("tau")
local t = tau.test
local g = t.gen
local scan = require("scan")

local function skill(name, description, rest)
	return "---\nname: " .. name .. "\ndescription: " .. description .. "\n" .. (rest or "") .. "---\n\nDo it.\n"
end

t.case("a folder with a SKILL.md and a description is a skill", function()
	local r = t.run({
		files = {
			["/s/release-notes/SKILL.md"] = skill("release-notes", "Writes release notes", "allowed-tools: Bash\n"),
			["/s/release-notes/scripts/tags.sh"] = "git tag",
			["/s/empty/SKILL.md"] = "---\nname: empty\n---\n",
			["/s/.hidden/SKILL.md"] = skill("hidden", "x"),
			["/s/plain/SKILL.md"] = "no frontmatter",
			["/s/Bad/SKILL.md"] = skill("Bad", "x"),
			["/s/twin/SKILL.md"] = skill("release-notes", "again"),
		},
		roots = { skills = "/s" },
	})
	local app = r:app()
	local skills = app.skills
	t.equal(#skills.found, 1)
	local found = skills.found[1]
	t.equal(found.name, "release-notes")
	t.equal(found.description, "Writes release notes")
	t.equal(found.files, 2)
	t.equal(found.ignored, { "allowed-tools" })
	local reasons = {}
	for _, problem in ipairs(skills.problems) do
		reasons[string.match(problem.dir, "([^/]+)$")] = problem.reason
	end
	t.equal(reasons.empty, "SKILL.md has no description in its frontmatter; the model picks skills by their description")
	t.equal(reasons.plain, "SKILL.md does not start with frontmatter (---)")
	t.equal(reasons.Bad, 'its name, "Bad", is not lower-case letters, digits and hyphens, up to 64')
	t.equal(reasons.twin, "another skill is named release-notes; this one is not offered")
	t.equal(reasons[".hidden"], nil)
	t.equal(app.nav, { detail = "1 skill", badge = "4", tone = "bad" })
end)

t.case("a skill of the person's is offered over one tau ships", function()
	local r = t.run({
		files = {
			["/mine/tdd/SKILL.md"] = skill("tdd", "Mine"),
			["/shipped/tdd/SKILL.md"] = skill("tdd", "Shipped"),
			["/shipped/architect/SKILL.md"] = skill("architect", "Shipped too"),
		},
		roots = { skills = "/mine", ["package/skills"] = "/shipped" },
	})
	local skills = r:app().skills
	local names = {}
	for _, found in ipairs(skills.found) do
		table.insert(names, found.name .. ":" .. found.description .. ":" .. tostring(found.builtin))
	end
	t.equal(names, { "architect:Shipped too:true", "tdd:Mine:false" })
end)

t.case("the instructions list each skill, principles and playbooks apart", function()
	local section = scan.section({
		found = {
			{ name = "playbook-feature", description = "Ship a feature" },
			{ name = "principle-small", description = "Keep it small" },
			{ name = "tdd", description = "Test first" },
		},
	})
	t.equal(string.sub(section, 1, 10), "# Skills\n\n")
	t.equal(string.find(section, "\n- tdd: Test first\n\n## Principles\n\n", 1, true) ~= nil, true, section)
	t.equal(string.find(section, "\n- principle-small: Keep it small\n\n## Playbooks\n\n", 1, true) ~= nil, true, section)
	t.equal(string.sub(section, -35), "\n- playbook-feature: Ship a feature")
	t.equal(scan.section({ found = {} }), nil)
end)

t.property("a description is one line, its white space one space, at most 1024 characters", function(draw)
	local text = draw(g.text({ max = 1200 }))
	local line = scan.one_line(text)
	t.equal(string.find(line, "[\n\t\r]") == nil, true, "one line")
	t.equal(string.find(line, "  ") == nil, true, "single spaces")
	t.equal(utf8.len(line) <= scan.DESCRIPTION_LIMIT + 1, true, "cut")
end)

t.property("a name is a skill's when it is lower-case words of letters and digits", function(draw)
	local name = draw(g.from_regex("[a-z0-9]{1,8}(-[a-z0-9]{1,8}){0,3}"))
	t.equal(scan.check_name(name), nil)
	t.equal(scan.check_name(name .. "-") ~= nil, true)
	t.equal(scan.check_name(string.upper(name) .. "X") ~= nil, true)
end)
