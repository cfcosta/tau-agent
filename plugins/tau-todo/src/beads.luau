-- Issues in beads' JSONL (ADR 0048): one issue a line, sorted by id,
-- each line's keys in one order, so the same issues give the same bytes
-- and the file diffs well in git. `import(export(issues))` gives the
-- issues back, every field.
--
-- beads keeps a link on the issue that depends: `blocks` (on what blocks
-- it), `parent-child` (on its epic), `discovered-from` (on what was
-- being worked when it was found) and `related`. "Done when" criteria
-- are its `acceptance_criteria`, a Markdown checklist; a stored blocked
-- status says what it waits on in `notes`.

local time = require("time")

local beads = {}

local TO_BEADS = { blocks = "blocks", parent = "parent-child", found_while = "discovered-from", related = "related" }
local FROM_BEADS = {}
for ours, theirs in pairs(TO_BEADS) do
	FROM_BEADS[theirs] = ours
end

local function str(value)
	return json.encode(value)
end

-- `fields`, pairs of a key and its JSON text, as an object in that
-- order; a nil text leaves its key out.
local function object(fields)
	local parts = {}
	for _, field in ipairs(fields) do
		if field[2] ~= nil then
			table.insert(parts, str(field[1]) .. ":" .. field[2])
		end
	end
	return "{" .. table.concat(parts, ",") .. "}"
end

local function list(items, each)
	local parts = {}
	for _, item in ipairs(items or {}) do
		table.insert(parts, each(item))
	end
	return "[" .. table.concat(parts, ",") .. "]"
end

local function criteria(issue)
	if #(issue.criteria or {}) == 0 then
		return nil
	end
	local lines = {}
	for _, criterion in ipairs(issue.criteria) do
		table.insert(lines, (criterion.done and "- [x] " or "- [ ] ") .. criterion.text)
	end
	return table.concat(lines, "\n")
end

local function line(issue)
	local n = 0
	return object({
		{ "id", str(issue.id) },
		{ "title", str(issue.title) },
		{ "description", str(issue.description or "") },
		{ "acceptance_criteria", criteria(issue) and str(criteria(issue)) },
		{ "notes", issue.blocked and str(issue.blocked) },
		{ "status", str(issue.status) },
		{ "priority", tostring(issue.priority) },
		{ "issue_type", str(issue.type) },
		{ "assignee", issue.assignee and str(issue.assignee) },
		{ "created_at", str(time.iso(issue.created)) },
		{ "updated_at", str(time.iso(issue.updated)) },
		{ "closed_at", issue.closed and str(time.iso(issue.closed)) },
		{ "close_reason", issue.close_reason and str(issue.close_reason) },
		{ "defer_until", issue.defer_until and str(time.iso(issue.defer_until)) },
		{ "labels", list(issue.labels, str) },
		{
			"dependencies",
			list(issue.links, function(link)
				return object({
					{ "issue_id", str(issue.id) },
					{ "depends_on_id", str(link.id) },
					{ "type", str(TO_BEADS[link.kind]) },
				})
			end),
		},
		{
			"comments",
			list(issue.comments, function(comment)
				n += 1
				return object({
					{ "id", tostring(n) },
					{ "issue_id", str(issue.id) },
					{ "author", str(comment.by) },
					{ "text", str(comment.text) },
					{ "created_at", str(time.iso(comment.at)) },
				})
			end),
		},
	})
end

-- `issues`, by id, as beads' JSONL.
function beads.export(issues)
	local ids = {}
	for id in pairs(issues) do
		table.insert(ids, id)
	end
	table.sort(ids)
	local lines = {}
	for _, id in ipairs(ids) do
		table.insert(lines, line(issues[id]))
	end
	if #lines == 0 then
		return ""
	end
	return table.concat(lines, "\n") .. "\n"
end

local function when(text, what, id)
	local unix = time.parse(text)
	if unix == nil then
		error(id .. ": `" .. what .. "` is not a time: " .. tostring(text), 0)
	end
	return unix
end

local function read_criteria(text)
	local out = {}
	if type(text) ~= "string" or text == "" then
		return out
	end
	for row in string.gmatch(text .. "\n", "(.-)\n") do
		local mark, rest = string.match(row, "^%s*[-*] %[([ xX])%] (.*)$")
		if mark then
			table.insert(out, { text = rest, done = mark ~= " " })
		elseif string.match(row, "%S") then
			table.insert(out, { text = (string.gsub(row, "^%s*[-*] ", "")), done = false })
		end
	end
	return out
end

local STATUSES = { open = true, in_progress = true, blocked = true, deferred = true, closed = true }
local TYPES = { bug = true, feature = true, task = true, epic = true, chore = true }

-- beads' JSONL `text` as issues by id; an issue beads tombstoned is left
-- out. Errors name the line that is wrong.
function beads.import(text)
	local issues = {}
	local at = 0
	for row in string.gmatch(text, "([^\n]*)\n?") do
		at += 1
		if string.match(row, "%S") then
			local ok, raw = pcall(json.decode, row)
			if not ok or type(raw) ~= "table" then
				error("line " .. at .. " is not an issue", 0)
			end
			local id = tostring(raw.id or ("line " .. at))
			if raw.status ~= "tombstone" then
				if type(raw.id) ~= "string" or type(raw.title) ~= "string" then
					error("line " .. at .. ": an issue needs its id and title", 0)
				end
				local status = raw.status or "open"
				if not STATUSES[status] then
					error(id .. ": no status `" .. tostring(status) .. "`", 0)
				end
				local kind = raw.issue_type or "task"
				if not TYPES[kind] then
					error(id .. ": no type `" .. tostring(kind) .. "`", 0)
				end
				local issue = {
					id = raw.id,
					title = raw.title,
					description = raw.description or "",
					criteria = read_criteria(raw.acceptance_criteria),
					type = kind,
					priority = math.clamp(math.floor(tonumber(raw.priority) or 2), 0, 4),
					status = status,
					labels = {},
					comments = {},
					links = {},
					created = when(raw.created_at, "created_at", id),
					updated = when(raw.updated_at or raw.created_at, "updated_at", id),
				}
				if status == "blocked" and type(raw.notes) == "string" and raw.notes ~= "" then
					issue.blocked = raw.notes
				end
				-- A person, not a tau run: kept as it is, it claims nothing
				-- (a claim is a run's, while it goes on).
				if raw.assignee and raw.assignee ~= "" then
					issue.assignee = tostring(raw.assignee)
				end
				if raw.closed_at then
					issue.closed = when(raw.closed_at, "closed_at", id)
				end
				if raw.close_reason and raw.close_reason ~= "" then
					issue.close_reason = raw.close_reason
				end
				if raw.defer_until then
					issue.defer_until = when(raw.defer_until, "defer_until", id)
				end
				for _, label in ipairs(raw.labels or {}) do
					table.insert(issue.labels, tostring(label))
				end
				for _, dep in ipairs(raw.dependencies or {}) do
					local kind = FROM_BEADS[dep.type or "blocks"]
					if kind and dep.depends_on_id then
						table.insert(issue.links, { id = tostring(dep.depends_on_id), kind = kind })
					end
				end
				for _, comment in ipairs(raw.comments or {}) do
					table.insert(issue.comments, {
						at = when(comment.created_at, "a comment's created_at", id),
						by = tostring(comment.author or ""),
						text = tostring(comment.text or ""),
					})
				end
				issues[issue.id] = issue
			end
		end
	end
	return issues
end

return beads
