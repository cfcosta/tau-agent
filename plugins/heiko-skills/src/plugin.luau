-- heiko-skills: instructions the agent loads when a task calls for them.
-- A skill is a folder in the person's skills folder (`~/.agents/skills`)
-- holding a `SKILL.md`: YAML frontmatter with a `name` and a
-- `description`, then the instructions. A run's instructions list each
-- skill by name and description; the model loads one with the `skill`
-- tool when a task fits it, and reads or runs the folder's other files
-- with the usual tools. heiko ships skills of its own, in this package and
-- those other plugins put in heiko's data folder; a skill of the person's
-- with the same name is offered instead.
local heiko = require("heiko")
local ui = heiko.ui
local cmd = heiko.cmd
local scan = require("scan")

-- What the model reads of the tool.
local DESCRIPTION = "Loads a skill: its instructions, from its SKILL.md, and the folder its other files are in. Load one when a task fits its description in the instructions' list, before starting."

-- The person's skills, and the folders of those heiko ships, as the host
-- has them.
local PERSONS = "skills"
local SHIPPED = { "package/skills", "data/skills" }

local function skills_of()
	local roots = heiko.await(cmd.files.roots())
	local shipped = {}
	for _, folder in ipairs(SHIPPED) do
		if roots[folder] then
			table.insert(shipped, roots[folder])
		end
	end
	return scan.all(roots[PERSONS], shipped)
end

-- The page's head: its title, how many skills there are, where they
-- are read from, and the way to open that folder.
local function header(skills)
	local actions = {}
	if skills.dir then
		table.insert(actions, ui.toned(skills.dir, "neutral", true))
		table.insert(actions, ui.button("open folder", "heiko.reveal", { path = skills.dir }, "secondary"))
	end
	return ui.header("Skills", { meta = tostring(#skills.found), actions = actions })
end

-- A skill as a ruled row: its name, what it is for, and its files.
local function row(skill)
	local files = heiko.count.plural(skill.files, "file")
	if skill.builtin then
		files ..= " · built in"
	end
	local about = { ui.text(skill.description) }
	if #skill.ignored > 0 then
		table.insert(about, ui.toned("ignores " .. table.concat(skill.ignored, ", "), "warn"))
	end
	return { ui.mono(skill.name), ui.stack(about), ui.toned(files, "neutral", true) }
end

-- A folder that is no skill: its name, why, and that the model is not
-- offered it.
local function problem_row(problem)
	return {
		ui.toned(string.match(problem.dir, "([^/]+)$") or problem.dir, "bad", true),
		ui.stack({ ui.text(problem.reason), ui.mono(problem.dir) }),
		ui.stamp("not offered", "bad"),
	}
end

local function list(skills)
	if #skills.found == 0 and #skills.problems == 0 then
		return ui.empty("No skills yet. A skill is a folder with a " .. scan.SKILL_FILE .. ".")
	end
	local rows = {}
	for _, skill in ipairs(skills.found) do
		table.insert(rows, row(skill))
	end
	for _, problem in ipairs(skills.problems) do
		table.insert(rows, problem_row(problem))
	end
	return ui.table({}, rows, { widths = { 2, 7, 1.5 } })
end

return heiko.plugin({
	uses = { files = { PERSONS, "package/skills", "data/skills" } },
	nav = { label = "Skills", icon = "skill" },

	-- Lists the skills in the run's instructions, as the folders hold them
	-- as it starts: a skill added meanwhile is listed the next time.
	start = function(_, model)
		local skills = skills_of()
		local listed = {}
		for _, skill in ipairs(skills.found) do
			listed[skill.name] = { dir = skill.dir, description = skill.description }
		end
		model.skills = listed
		model.order = array({})
		for _, skill in ipairs(skills.found) do
			table.insert(model.order, skill.name)
		end
		local section = scan.section(skills)
		if section == nil then
			return nil
		end
		return heiko.start({ instructions = section })
	end,

	tools = {
		[scan.TOOL] = {
			kind = "look",
			description = DESCRIPTION,
			parameters = {
				type = "object",
				properties = {
					name = { type = "string", description = "The skill's name, as the list gives it." },
				},
				required = { "name" },
				additionalProperties = false,
			},
			-- A listed skill's instructions, as its SKILL.md has them now.
			call = function(args, model, env)
				local name = tostring(args.name or "")
				local skill = (model.skills or {})[name]
				if skill == nil then
					error(
						"There is no skill "
							.. scan.quoted(name)
							.. ". The skills are: "
							.. table.concat(model.order or {}, ", ")
							.. ".",
						0
					)
				end
				local file = skill.dir .. "/" .. scan.SKILL_FILE
				local text = heiko.await(cmd.files.read(file))
				local front = heiko.await(cmd.files.frontmatter(file))
				local body = front.body or text
				local instructions = heiko.text.trim(body)
				-- Read after the awaits: another call may have loaded one meanwhile.
				model.loaded = model.loaded or {}
				model.loaded[env.run.call] = {
					name = name,
					description = skill.description,
					file = file,
					instructions = instructions,
				}
				return "Skill "
					.. name
					.. ". Its folder, where the files it names are: "
					.. skill.dir
					.. "\n\n"
					.. instructions
			end,
		},
	},

	-- Each loaded skill's line: which, and what it is for. A click opens
	-- what the model read: its instructions as written.
	view = function(model)
		local calls = {}
		for call, loaded in pairs(model.loaded or {}) do
			local _, breaks = string.gsub(loaded.instructions, "\n", "")
			calls[call] = {
				head = ui.line({ ui.mono(loaded.name), ui.toned(loaded.description, "neutral") }),
				label = breaks == 0 and "1 line" or ((breaks + 1) .. " lines"),
				body = ui.code("markdown", loaded.instructions),
				folds = true,
			}
		end
		if next(calls) == nil then
			return {}
		end
		return { calls = calls }
	end,

	-- What the folders hold, for the sidebar, the page and the commands:
	-- how many skills, and a badge for the folders that are not one.
	app = function()
		local skills = skills_of()
		local problems = #skills.problems
		return {
			skills = skills,
			nav = {
				detail = heiko.count.plural(#skills.found, "skill"),
				badge = problems > 0 and tostring(problems) or nil,
				tone = problems > 0 and "bad" or nil,
			},
		}
	end,

	-- The screen: its header, then the skills and the folders that are not.
	page = function(app)
		return ui.stack({ header(app.skills), list(app.skills) })
	end,

	-- Each skill but the principles as `/name` in the composer's menu,
	-- with what to do after it; sending it asks the model to load the
	-- skill first. A principle still loads when a message names it.
	commands = {
		list = function(app)
			local listed = {}
			for _, skill in ipairs(app.skills.found) do
				if string.sub(skill.name, 1, #scan.PRINCIPLE) ~= scan.PRINCIPLE then
					listed[skill.name] = { description = skill.description, args = "what to do", icon = "skill" }
				end
			end
			return listed
		end,
		run = function(name, text)
			return heiko.interface("send", { text = text == "" and ("/" .. name) or ("/" .. name .. " " .. text) })
		end,
	},
})
