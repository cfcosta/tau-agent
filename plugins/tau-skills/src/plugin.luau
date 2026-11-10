-- tau-skills: instructions the agent loads when a task calls for them.
-- A skill is a folder in the person's skills folder (`~/.agents/skills`)
-- holding a `SKILL.md`: YAML frontmatter with a `name` and a
-- `description`, then the instructions. A run's instructions list each
-- skill by name and description; the model loads one with the `skill`
-- tool when a task fits it, and reads or runs the folder's other files
-- with the usual tools. tau ships skills of its own, in this package and
-- those other plugins put in tau's data folder; a skill of the person's
-- with the same name is offered instead.
local tau = require("tau")
local ui = tau.ui
local scan = require("scan")

-- What the model reads of the tool.
local DESCRIPTION = "Loads a skill: its instructions, from its SKILL.md, and the folder its other files are in. Load one when a task fits its description in the instructions' list, before starting."

-- The person's skills, and the folders of those tau ships, as the host
-- has them.
local PERSONS = "skills"
local SHIPPED = { "package/skills", "data/skills" }

local function skills_of(ctx)
	local roots = ctx.files.roots()
	local shipped = {}
	for _, folder in ipairs(SHIPPED) do
		if roots[folder] then
			table.insert(shipped, roots[folder])
		end
	end
	return scan.all(ctx.files, roots[PERSONS], shipped)
end

local function plural(n, one, many)
	return n == 1 and ("1 " .. one) or (tostring(n) .. " " .. many)
end

-- The page's header: its title and the folder's button, what skills are,
-- and where they are read from.
local function header(skills)
	local title = { ui.heading("Skills"), ui.spacer() }
	if skills.dir then
		table.insert(title, ui.button("Open folder", "tau.reveal", { path = skills.dir }, "secondary"))
	end
	local rows = {
		ui.row(title),
		ui.text(
			"Instructions the model loads when a task calls for them, or that you start with /name. Each is a folder with a "
				.. scan.SKILL_FILE
				.. "."
		),
	}
	if skills.dir then
		table.insert(
			rows,
			ui.row({
				ui.text("From"),
				ui.mono(skills.dir),
				ui.text("read as each run starts; new ones show in the next run"),
			})
		)
	end
	return ui.stack(rows)
end

local function row(skill)
	local files = plural(skill.files, "file", "files")
	if skill.builtin then
		files ..= " · built in"
	end
	local children = {
		ui.row({ ui.mono(skill.name), ui.spacer(), ui.mono(files) }),
		ui.text(skill.description),
	}
	if #skill.ignored > 0 then
		table.insert(
			children,
			ui.text("Also sets " .. table.concat(skill.ignored, ", ") .. ", which tau does not act on yet.")
		)
	end
	return ui.card(children)
end

local function problem_row(problem)
	return ui.card({
		ui.row({
			ui.icon("warning", "bad"),
			ui.mono(string.match(problem.dir, "([^/]+)$") or problem.dir),
			ui.badge("not offered to the model", "bad"),
		}),
		ui.text(problem.reason),
		ui.mono(problem.dir),
	})
end

local function list(skills)
	if #skills.found == 0 and #skills.problems == 0 then
		return ui.empty(
			"No skills yet. A skill is a folder here with a "
				.. scan.SKILL_FILE
				.. ": YAML frontmatter with a name and a description, then the instructions."
		)
	end
	local rows = {}
	for _, skill in ipairs(skills.found) do
		table.insert(rows, row(skill))
	end
	for _, problem in ipairs(skills.problems) do
		table.insert(rows, problem_row(problem))
	end
	return ui.stack(rows)
end

return tau.plugin({
	uses = { files = { PERSONS, "package/skills", "data/skills" } },
	nav = { label = "Skills", icon = "skill" },

	-- Lists the skills in the run's instructions, as the folders hold them
	-- as it starts: a skill added meanwhile is listed the next time.
	start = function(_, ctx)
		local skills = skills_of(ctx)
		local listed = {}
		for _, skill in ipairs(skills.found) do
			listed[skill.name] = { dir = skill.dir, description = skill.description }
		end
		ctx.state.skills = listed
		ctx.state.order = array({})
		for _, skill in ipairs(skills.found) do
			table.insert(ctx.state.order, skill.name)
		end
		local section = scan.section(skills)
		if section == nil then
			return nil
		end
		return tau.start({ instructions = section })
	end,

	tools = {
		[scan.TOOL] = {
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
			call = function(args, ctx)
				local name = tostring(args.name or "")
				local skill = (ctx.state.skills or {})[name]
				if skill == nil then
					error(
						"There is no skill "
							.. scan.quoted(name)
							.. ". The skills are: "
							.. table.concat(ctx.state.order or {}, ", ")
							.. ".",
						0
					)
				end
				local file = skill.dir .. "/" .. scan.SKILL_FILE
				local text = ctx.files.read(file)
				local front = ctx.files.frontmatter(file)
				local body = front.body or text
				local instructions = scan.trim(body)
				ctx.state.loaded = ctx.state.loaded or {}
				ctx.state.loaded[ctx.run.call] = {
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

	-- Each loaded skill's card: which, what it is for, and what the model
	-- read, the file and its instructions.
	view = function(state)
		local calls = {}
		for call, loaded in pairs(state.loaded or {}) do
			calls[call] = {
				head = ui.row({ ui.icon("skill", "info"), ui.mono(loaded.name), ui.text(loaded.description) }),
				body = ui.stack({ ui.mono(loaded.file), ui.text(loaded.instructions) }),
			}
		end
		if next(calls) == nil then
			return {}
		end
		return { calls = calls }
	end,

	-- What the folders hold, for the sidebar, the page and the commands:
	-- how many skills, and a badge for the folders that are not one.
	app = function(ctx)
		local skills = skills_of(ctx)
		local problems = #skills.problems
		return {
			skills = skills,
			nav = {
				detail = plural(#skills.found, "skill", "skills"),
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
			return tau.interface("send", { text = text == "" and ("/" .. name) or ("/" .. name .. " " .. text) })
		end,
	},
})
