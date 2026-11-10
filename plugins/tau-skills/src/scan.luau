-- Reading the skills folders: each folder in one with a `SKILL.md`, its
-- frontmatter, and what a run's instructions say of them. `files` is a
-- hook's `ctx.files`.
local scan = {}

-- The file in a skill's folder that holds its frontmatter and
-- instructions.
scan.SKILL_FILE = "SKILL.md"

-- A name the model loads a skill by: lower-case letters, digits and
-- hyphens, as other agents' skills are named, at most this long.
scan.NAME_LIMIT = 64

-- How much of a description the instructions take, in characters.
scan.DESCRIPTION_LIMIT = 1024

-- How many files a skill's folder is counted up to: more say "many".
scan.FILES_LIMIT = 1000

-- The heading the list goes under in a run's instructions.
scan.HEADING = "# Skills"

-- The prefix of a principle's name. Principles are listed apart in a
-- run's instructions, and left out of the composer's menu: the model
-- applies them, and the person rarely asks for one by name.
scan.PRINCIPLE = "principle-"

-- The tool's name, as the model calls it.
scan.TOOL = "skill"

-- What Unicode calls white space past ASCII's, by code point: what a
-- description's runs of white space are made of too.
local WIDE_SPACE = {
	[0x85] = true,
	[0xA0] = true,
	[0x1680] = true,
	[0x2028] = true,
	[0x2029] = true,
	[0x202F] = true,
	[0x205F] = true,
	[0x3000] = true,
}
for point = 0x2000, 0x200A do
	WIDE_SPACE[point] = true
end

local function is_space(point)
	return point == 0x20 or (point >= 0x09 and point <= 0x0D) or WIDE_SPACE[point] == true
end

-- `text` on one line, its runs of white space one space, cut to
-- `DESCRIPTION_LIMIT` characters.
function scan.one_line(text)
	local words = {}
	local word = {}
	for _, point in utf8.codes(text) do
		if is_space(point) then
			if #word > 0 then
				table.insert(words, table.concat(word))
				word = {}
			end
		else
			table.insert(word, utf8.char(point))
		end
	end
	if #word > 0 then
		table.insert(words, table.concat(word))
	end
	local joined = table.concat(words, " ")
	if utf8.len(joined) > scan.DESCRIPTION_LIMIT then
		return string.sub(joined, 1, utf8.offset(joined, scan.DESCRIPTION_LIMIT + 1) - 1) .. "…"
	end
	return joined
end

-- `text` with its white space at both ends gone.
function scan.trim(text)
	local points = {}
	for _, point in utf8.codes(text) do
		table.insert(points, point)
	end
	local first, last = 1, #points
	while first <= last and is_space(points[first]) do
		first += 1
	end
	while last >= first and is_space(points[last]) do
		last -= 1
	end
	local kept = {}
	for at = first, last do
		table.insert(kept, utf8.char(points[at]))
	end
	return table.concat(kept)
end

-- `text` quoted as Rust's `{:?}` writes a string: the quotes, backslash
-- and control characters escaped.
function scan.quoted(text)
	local out = {}
	for _, point in utf8.codes(text) do
		if point == 0x22 then
			table.insert(out, '\\"')
		elseif point == 0x5C then
			table.insert(out, "\\\\")
		elseif point == 0x0A then
			table.insert(out, "\\n")
		elseif point == 0x0D then
			table.insert(out, "\\r")
		elseif point == 0x09 then
			table.insert(out, "\\t")
		elseif point == 0x00 then
			table.insert(out, "\\0")
		elseif point < 0x20 or point == 0x7F then
			table.insert(out, string.format("\\u{%x}", point))
		else
			table.insert(out, utf8.char(point))
		end
	end
	return '"' .. table.concat(out) .. '"'
end

-- Whether `name` is one a skill can have: nil, or why not.
function scan.check_name(name)
	local fits = #name > 0
		and #name <= scan.NAME_LIMIT
		and string.find(name, "^[a-z0-9%-]+$") ~= nil
		and string.sub(name, 1, 1) ~= "-"
		and string.sub(name, -1) ~= "-"
	if fits then
		return nil
	end
	return string.format(
		"its name, %s, is not lower-case letters, digits and hyphens, up to %d",
		scan.quoted(name),
		scan.NAME_LIMIT
	)
end

-- What `ctx.files` raised, past the path it names: "Cannot read <path>:
-- <why>" says <why>.
local function why_of(raised)
	local text = tostring(raised)
	return string.match(text, "^Cannot read .-: (.*)$") or text
end

-- The files under `folder`, counted up to `FILES_LIMIT`.
local function count_files(files, folder)
	local count = 0
	local todo = { folder }
	while #todo > 0 do
		local dir = table.remove(todo)
		local ok, entries = pcall(files.list, dir)
		if ok then
			for _, entry in ipairs(entries) do
				if entry.dir then
					table.insert(todo, dir .. "/" .. entry.name)
				else
					count += 1
					if count >= scan.FILES_LIMIT then
						return count
					end
				end
			end
		end
	end
	return count
end

-- One skill's folder, `name` in `dir`: the skill, or nil and why it is
-- not one.
function scan.read(files, dir, name)
	local folder = dir .. "/" .. name
	local ok, front = pcall(files.frontmatter, folder .. "/" .. scan.SKILL_FILE)
	if not ok then
		return nil, "cannot read " .. scan.SKILL_FILE .. ": " .. why_of(front)
	end
	if front.error ~= nil then
		return nil, front.error
	end
	local given = {}
	local ignored = {}
	for _, key in ipairs(front.keys or {}) do
		if key.key == "name" or key.key == "description" then
			if key.kind == "other" then
				return nil, "its " .. key.key .. " is not text"
			end
			given[key.key] = key.kind ~= "null" and key.text or nil
		else
			table.insert(ignored, key.key)
		end
	end
	local skill_name = given.name or name
	local wrong = scan.check_name(skill_name)
	if wrong then
		return nil, wrong
	end
	local description = given.description and scan.one_line(given.description) or ""
	if description == "" then
		return nil,
			scan.SKILL_FILE
				.. " has no description in its frontmatter; the model picks skills by their description"
	end
	return {
		name = skill_name,
		description = description,
		dir = folder,
		files = count_files(files, folder),
		ignored = array(ignored),
		builtin = false,
	}
end

-- `dir`'s skills: every folder in it, by name, with a `SKILL.md` that has
-- a description. A folder without one, or with a name another skill
-- has, is a problem. A missing folder holds nothing.
function scan.scan(files, dir)
	local skills = { dir = dir, found = array({}), problems = array({}) }
	local ok, entries = pcall(files.list, dir)
	if not ok then
		return skills
	end
	local folders = {}
	for _, entry in ipairs(entries) do
		-- A link to a folder counts: skills are often linked in.
		if entry.dir and string.sub(entry.name, 1, 1) ~= "." then
			table.insert(folders, entry.name)
		end
	end
	table.sort(folders)
	local names = {}
	for _, name in ipairs(folders) do
		local skill, why = scan.read(files, dir, name)
		if skill and not names[skill.name] then
			names[skill.name] = true
			table.insert(skills.found, skill)
		elseif skill then
			table.insert(skills.problems, {
				dir = dir .. "/" .. name,
				reason = "another skill is named " .. skill.name .. "; this one is not offered",
			})
		else
			table.insert(skills.problems, { dir = dir .. "/" .. name, reason = why })
		end
	end
	table.sort(skills.found, function(a, b)
		return a.name < b.name
	end)
	return skills
end

-- The person's skills in `dir`, and those tau ships in `builtins`, each
-- folder in turn, that the person has none of the same name of.
function scan.all(files, dir, builtins)
	local skills = dir and scan.scan(files, dir) or { dir = nil, found = array({}), problems = array({}) }
	local mine = {}
	for _, skill in ipairs(skills.found) do
		mine[skill.name] = true
	end
	local shipped = {}
	for _, folder in ipairs(builtins) do
		local found = scan.scan(files, folder)
		for _, skill in ipairs(found.found) do
			if shipped[skill.name] then
				-- Two folders tau ships with one name: as a folder holding
				-- both would say.
				table.insert(found.problems, {
					dir = skill.dir,
					reason = "another skill is named " .. skill.name .. "; this one is not offered",
				})
			elseif not mine[skill.name] then
				shipped[skill.name] = true
				skill.builtin = true
				table.insert(skills.found, skill)
			else
				shipped[skill.name] = true
			end
		end
		for _, problem in ipairs(found.problems) do
			table.insert(skills.problems, problem)
		end
	end
	table.sort(skills.found, function(a, b)
		return a.name < b.name
	end)
	return skills
end

-- The kinds of skill a run's instructions list apart, by the prefix of
-- their names: each kind's heading, and what it tells the model to do.
-- The rest are listed first, under none.
scan.KINDS = {
	{
		prefix = scan.PRINCIPLE,
		heading = "## Principles",
		intro = "Engineering principles. Each line says when the principle applies and what it asks. Before you apply one, load its skill and read it in full. When a principle changed a decision, say which in your answer.",
	},
	{
		prefix = "playbook-",
		heading = "## Playbooks",
		intro = "Procedures for kinds of task. When a task is one of these, load its playbook before you start, copy its steps into your notes, and work through them in order; mark a step you skip with why.",
	},
}

local function kind_of(skill)
	for at, kind in ipairs(scan.KINDS) do
		if string.sub(skill.name, 1, #kind.prefix) == kind.prefix then
			return at
		end
	end
	return nil
end

-- What a run's instructions say of `skills`: how to load one, then each
-- by name and description, the kinds under their headings. Nil when
-- there are none.
function scan.section(skills)
	if #skills.found == 0 then
		return nil
	end
	local text = scan.HEADING
		.. "\n\n"
		.. "Skills are instructions for particular tasks, kept in folders outside the workspace. When a task fits a skill's description, call `"
		.. scan.TOOL
		.. "` with its name before you start, and follow what it says. Its folder may hold scripts and references it names: read and run them from there. A message that starts with `/name` asks for that skill: load it first, then do what the message says after the name.\n"
	local function line(skill)
		return "\n- " .. skill.name .. ": " .. skill.description
	end
	for _, skill in ipairs(skills.found) do
		if kind_of(skill) == nil then
			text ..= line(skill)
		end
	end
	for at, kind in ipairs(scan.KINDS) do
		local listed = {}
		for _, skill in ipairs(skills.found) do
			if kind_of(skill) == at then
				table.insert(listed, skill)
			end
		end
		if #listed > 0 then
			text = string.gsub(text, "%s+$", "") .. "\n\n" .. kind.heading .. "\n\n" .. kind.intro .. "\n"
			for _, skill in ipairs(listed) do
				text ..= line(skill)
			end
		end
	end
	return text
end

return scan
