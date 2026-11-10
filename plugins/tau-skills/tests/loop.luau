-- tau-skills in a real run: the instructions list the person's skills,
-- and the tool gives the model one's instructions and its folder, or
-- says which skills there are when asked for one that is not.
local tau = require("tau")
local t = tau.test

local SKILLS = {
	["release-notes/SKILL.md"] = "---\nname: release-notes\ndescription: Writes release notes between two tags\n---\n\nGroup the commits by kind.\n",
	["code-review/SKILL.md"] = "---\nname: code-review\ndescription: Reviews the diff\n---\n\nLook for bugs.\n",
}

local function has(text, part)
	return string.find(text, part, 1, true) ~= nil
end

t.loop("the model loads a listed skill", {
	skills = SKILLS,
	model = {
		{ tool = "skill", args = { name = "release-notes" } },
		{ text = "Grouped." },
	},
}, function(run)
	-- The instructions list both, and keep the skills' own text out.
	local instructions = run.requests[1].instructions
	t.equal(has(instructions, "# Skills"), true, instructions)
	t.equal(has(instructions, "- code-review: Reviews the diff"), true)
	t.equal(has(instructions, "- release-notes: Writes release notes between two tags"), true)
	t.equal(has(instructions, "Group the commits"), false)
	-- The call gives its instructions, without the frontmatter, and its
	-- folder, for the files it names.
	t.equal(#run.results, 1)
	local result = run.results[1]
	t.equal(result.error, false, result.text)
	t.equal(has(result.text, "Group the commits by kind."), true)
	t.equal(has(result.text, "description:"), false)
	t.equal(has(result.text, "/skills/release-notes\n\n"), true, result.text)
	local _, loaded = next(run.state.loaded)
	t.equal(loaded.name, "release-notes")
	t.equal(string.sub(loaded.file, -#"/skills/release-notes/SKILL.md"), "/skills/release-notes/SKILL.md")
end)

t.loop("an unknown skill names the ones there are", {
	skills = SKILLS,
	model = {
		-- A name like a path does not reach outside the folder.
		{ tool = "skill", args = { name = "../code-review" } },
		{ text = "Sorry." },
	},
}, function(run)
	t.equal(#run.results, 1)
	local result = run.results[1]
	t.equal(result.error, true)
	t.equal(has(result.text, 'no skill "../code-review"'), true, result.text)
	t.equal(has(result.text, "The skills are: "), true, result.text)
	t.equal(has(result.text, "code-review, "), true, result.text)
	t.equal(has(result.text, "release-notes"), true, result.text)
end)
