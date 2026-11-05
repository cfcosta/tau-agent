-- A repository's issues in the plugin's store (ADR 0048), and every
-- change the tools and the board make to them. Each issue is one key,
-- `issue/<id>`, its links with it, so a change is one write; an id once
-- used is kept under `retired/<id>` when its issue goes, so it never
-- comes back. A change that fails raises its error, and the hook's
-- store keeps none of what it wrote.

local graph = require("graph")
local time = require("time")

local issues = {}

local PREFIX = "issue/"
local RETIRED = "retired/"

-- Every issue, by id.
function issues.all(store)
	local out = {}
	for _, kept in ipairs(store.list(PREFIX)) do
		out[kept.value.id] = kept.value
	end
	return out
end

-- Every id there is or was.
local function taken(store, all)
	local out = {}
	for id in pairs(all) do
		out[id] = true
	end
	for _, kept in ipairs(store.list(RETIRED)) do
		out[string.sub(kept.key, #RETIRED + 1)] = true
	end
	return out
end

local function fail(text)
	error(text, 0)
end

-- `id`'s issue in `all`, or an error saying there is none.
local function find(all, id)
	if type(id) ~= "string" then
		fail("name the issue by its id, such as td-4e1b")
	end
	local issue = all[id]
	if not issue then
		fail("there is no issue " .. id)
	end
	return issue
end
issues.find = find

local function save(store, issue, now)
	issue.updated = now
	store.put(PREFIX .. issue.id, issue)
end

-- `P0` to `P4`, or 0 to 4, as a number; nil for nil.
function issues.priority(value)
	if value == nil then
		return nil
	end
	local n = tonumber(value) or tonumber(string.match(tostring(value), "^[Pp](%d)$"))
	if n == nil or n ~= math.floor(n) or n < 0 or n > 4 then
		fail("a priority is P0 (drop everything) to P4 (some day), not " .. tostring(value))
	end
	return n
end

local function kind(value)
	if value == nil then
		return nil
	end
	if not graph.TYPES[value] then
		fail("a type is bug, feature, task, epic or chore, not " .. tostring(value))
	end
	return value
end

local function strings(value, what)
	if value == nil then
		return nil
	end
	if type(value) ~= "table" then
		fail(what .. " are a list of texts")
	end
	local out = {}
	for _, item in ipairs(value) do
		if type(item) ~= "string" or item == "" then
			fail(what .. " are a list of texts")
		end
		table.insert(out, item)
	end
	return out
end

local function criteria(value)
	local texts = strings(value, "criteria")
	if texts == nil then
		return nil
	end
	local out = {}
	for _, text in ipairs(texts) do
		table.insert(out, { text = text, done = false })
	end
	return out
end

-- A new issue from `args`: `{ title, description?, type?, priority?,
-- labels?, criteria?, parent?, found_while? }`, made at `now`. A child
-- of an epic takes its id after the epic's.
function issues.create(store, now, args)
	if type(args.title) ~= "string" or args.title == "" then
		fail("an issue needs its title")
	end
	local all = issues.all(store)
	local ids = taken(store, all)
	local links = {}
	local id
	if args.parent ~= nil then
		local epic = find(all, args.parent)
		if epic.type ~= "epic" then
			fail(epic.id .. " is a " .. epic.type .. ", not an epic")
		end
		id = graph.child_id(ids, epic.id)
		table.insert(links, { id = epic.id, kind = "parent" })
	else
		id = graph.new_id(ids, args.title .. "\n" .. tostring(now))
	end
	if args.found_while ~= nil then
		table.insert(links, { id = find(all, args.found_while).id, kind = "found_while" })
	end
	local issue = {
		id = id,
		title = args.title,
		description = type(args.description) == "string" and args.description or "",
		criteria = criteria(args.criteria) or {},
		type = kind(args.type) or "task",
		priority = issues.priority(args.priority) or 2,
		status = "open",
		labels = strings(args.labels, "labels") or {},
		comments = {},
		links = links,
		created = now,
	}
	save(store, issue, now)
	return issue
end

-- Changes `args.id`'s title, description, type, priority, labels or
-- criteria, and its status through what may follow: closing has its own
-- call, for its reason.
function issues.update(store, now, args)
	local all = issues.all(store)
	local issue = find(all, args.id)
	if args.title ~= nil then
		if type(args.title) ~= "string" or args.title == "" then
			fail("a title is a text")
		end
		issue.title = args.title
	end
	if args.description ~= nil then
		issue.description = tostring(args.description)
	end
	issue.type = kind(args.type) or issue.type
	issue.priority = issues.priority(args.priority) or issue.priority
	issue.labels = strings(args.labels, "labels") or issue.labels
	if args.criteria ~= nil then
		issue.criteria = criteria(args.criteria)
	end
	if args.status ~= nil and args.status ~= issue.status then
		if args.status == "closed" then
			fail("close an issue with todo_close, saying why")
		end
		issues.move(issue, args.status, args.blocked)
	end
	save(store, issue, now)
	return issue
end

-- `issue` to `status`, when it may go there. A stored blocked status
-- says what it waits on; one that is no longer in progress is nobody's.
function issues.move(issue, status, blocked)
	local ok, why = graph.may_move(issue, status)
	if not ok then
		fail(why)
	end
	issue.status = status
	if status == "blocked" then
		issue.blocked = type(blocked) == "string" and blocked ~= "" and blocked or "something outside tau"
	else
		issue.blocked = nil
	end
	if status ~= "deferred" then
		issue.defer_until = nil
	end
	if status ~= "in_progress" then
		issue.claim = nil
	end
	if status ~= "closed" then
		issue.closed = nil
		issue.close_reason = nil
	end
end

-- `run` claims `id`: it goes in progress, the run's; refused when
-- another run holds it, or it is closed.
function issues.claim(store, now, id, run)
	local all = issues.all(store)
	local issue = find(all, id)
	if issue.claim == run then
		return issue
	end
	if issue.claim ~= nil then
		fail(id .. " is held by the run " .. issue.claim)
	end
	if issue.status == "closed" then
		fail(id .. " is closed: reopen it first")
	end
	issues.move(issue, "in_progress")
	issue.claim = run
	save(store, issue, now)
	return issue
end

-- Ends what `run` holds of `id`, which goes back to open, `note` said in
-- a comment by `by`; an issue it closed stays closed. Gives whether it
-- held it.
function issues.let_go(store, now, id, run, note, by)
	local issue = issues.all(store)[id]
	if not issue or issue.claim ~= run then
		return false
	end
	issue.claim = nil
	if issue.status == "in_progress" then
		issue.status = "open"
	end
	if note and note ~= "" then
		table.insert(issue.comments, { at = now, by = by or run, text = note })
	end
	save(store, issue, now)
	return true
end

-- The issues `run` holds, by id.
function issues.held_by(store, run)
	local out = {}
	for id, issue in pairs(issues.all(store)) do
		if issue.claim == run then
			table.insert(out, id)
		end
	end
	table.sort(out)
	return out
end

-- Checks, or unchecks, criterion `at` (from 1) of `id`.
function issues.check(store, now, id, at, done)
	local issue = find(issues.all(store), id)
	local criterion = (issue.criteria or {})[tonumber(at) or 0]
	if not criterion then
		fail(id .. " has " .. #(issue.criteria or {}) .. " criteria; there is no " .. tostring(at))
	end
	criterion.done = done ~= false
	save(store, issue, now)
	return issue
end

-- Adds a link from `id` of `kind` to `on`, or takes it away: `blocks`,
-- `on` blocks `id`; `parent`, `on` is the epic over `id`; `found_while`,
-- `id` was found while `on` was worked; `related`. A `blocks` link that
-- closes a cycle is refused, with the cycle.
function issues.link(store, now, id, kind, on, remove)
	if not graph.KINDS[kind] then
		fail("a link is blocks, parent, found_while or related, not " .. tostring(kind))
	end
	local all = issues.all(store)
	local issue = find(all, id)
	local other = find(all, on)
	local at
	for n, link in ipairs(issue.links) do
		if link.id == on and link.kind == kind then
			at = n
		end
	end
	if remove then
		if not at then
			fail(id .. " has no " .. kind .. " link to " .. on)
		end
		table.remove(issue.links, at)
	elseif not at then
		if kind == "blocks" then
			local ok, why = graph.may_block(all, on, id)
			if not ok then
				fail(why)
			end
		elseif kind == "parent" then
			if other.type ~= "epic" then
				fail(on .. " is a " .. other.type .. ", not an epic")
			end
			if #graph.links(issue, "parent") > 0 then
				fail(id .. " has an epic already")
			end
		elseif on == id then
			fail("an issue links to another")
		end
		table.insert(issue.links, { id = on, kind = kind })
	end
	save(store, issue, now)
	return issue
end

function issues.comment(store, now, id, text, by)
	if type(text) ~= "string" or text == "" then
		fail("a comment is a text")
	end
	local issue = find(issues.all(store), id)
	table.insert(issue.comments, { at = now, by = by, text = text })
	save(store, issue, now)
	return issue
end

-- Closes `id` for `reason`: an epic once its children have.
function issues.close(store, now, id, reason)
	if type(reason) ~= "string" or reason == "" then
		fail("closing says why")
	end
	local all = issues.all(store)
	local issue = find(all, id)
	local ok, why = graph.may_close(all, id)
	if not ok then
		fail(why)
	end
	issues.move(issue, "closed")
	issue.closed = now
	issue.close_reason = reason
	save(store, issue, now)
	return issue
end

function issues.reopen(store, now, id)
	local issue = find(issues.all(store), id)
	issues.move(issue, "open")
	save(store, issue, now)
	return issue
end

-- Defers `id`, until `until` when given: a date or a time.
function issues.defer(store, now, id, until_)
	local issue = find(issues.all(store), id)
	local at
	if until_ ~= nil then
		at = time.parse(until_)
		if at == nil then
			fail("defer until a date, such as 2026-11-10, not " .. tostring(until_))
		end
	end
	issues.move(issue, "deferred")
	issue.defer_until = at
	save(store, issue, now)
	return issue
end

-- Deletes `id`: its id is kept as used, and links to it go.
function issues.delete(store, now, id)
	local all = issues.all(store)
	find(all, id)
	for other, issue in pairs(all) do
		local kept = {}
		for _, link in ipairs(issue.links) do
			if link.id ~= id then
				table.insert(kept, link)
			end
		end
		if #kept ~= #issue.links then
			issue.links = kept
			save(store, issue, now)
		end
	end
	store.delete(PREFIX .. id)
	store.put(RETIRED .. id, true)
end

-- Every issue in `imported` put in the store, in place of one with its
-- id; gives how many.
function issues.import(store, now, imported)
	local n = 0
	for _, issue in pairs(imported) do
		store.put(PREFIX .. issue.id, issue)
		n += 1
	end
	return n
end

-- The issues whose title, description or comments hold `text`, any case.
function issues.search(all, text)
	local needle = string.lower(tostring(text or ""))
	local out = {}
	for id, issue in pairs(all) do
		local hay = { issue.title, issue.description or "" }
		for _, comment in ipairs(issue.comments or {}) do
			table.insert(hay, comment.text)
		end
		if string.find(string.lower(table.concat(hay, "\n")), needle, 1, true) then
			table.insert(out, id)
		end
	end
	return graph.by_priority(all, out)
end

return issues
