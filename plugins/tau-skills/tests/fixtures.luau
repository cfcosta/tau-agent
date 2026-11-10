-- The demo's fixtures: a run that loaded release-notes, its card's state
-- and view, from the demo's skills folder.
local tau = require("tau")
local t = tau.test

local FOLDER = "~/.agents/skills"

local FILES = {
	[FOLDER .. "/code-review/SKILL.md"] = "---\nname: code-review\ndescription: Reviews the current diff for correctness bugs, ranked by severity, with a failing input for each.\n---\n\nRead the diff against trunk. For each bug, say what input fails and how.\n",
	[FOLDER .. "/release-notes/SKILL.md"] = {
		text = "---\nname: release-notes\ndescription: Writes release notes from the commits between two tags: groups by kind, leads with what users notice.\n---\n\nRun scripts/commits.sh with the two tags, group what it lists, and lead with what users will notice.\n",
		frontmatter = {
			keys = {
				{
					key = "name",
					kind = "string",
					text = "release-notes",
				},
				{
					key = "description",
					kind = "string",
					text = "Writes release notes from the commits between two tags: groups by kind, leads with what users notice.",
				},
				{ key = "allowed-tools", kind = "other", text = json.null },
			},
			body = "\nRun scripts/commits.sh with the two tags, group what it lists, and lead with what users will notice.\n",
		},
	},
	[FOLDER .. "/release-notes/scripts/commits.sh"] = '#!/bin/sh\ngit log --oneline "$1..$2"\n',
}

t.fixture("skill", function()
	local r = t.run({ id = "demo", files = FILES, roots = { skills = FOLDER } })
	r:start({})
	local text = r:tool("skill", { name = "release-notes" }, "call_skill")
	return { text = text, state = r.state, view = r:view() }
end)
