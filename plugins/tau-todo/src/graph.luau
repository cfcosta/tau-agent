-- The issue graph (ADR 0048): issues, the links between them, what is
-- ready, cycles, epics, ids and the status a change may go to. Plain
-- functions over a table of issues by id: no store, no clock, so the
-- graph laws can call each with what they draw.
--
-- An issue is `{ id, title, description, criteria = { { text, done } },
-- type, priority, status, blocked, defer_until, close_reason, labels,
-- comments = { { at, by, text } }, created, updated, closed, claim,
-- links = { { id, kind } } }`. Times are seconds since 1970. A link is
-- kept on the issue it points from: `{ id = "td-1", kind = "blocks" }`
-- on td-2 says td-1 blocks td-2; `parent` names the epic, `found_while`
-- the issue being worked when this one was filed, `related` anything.

local graph = {}

graph.TYPES = { bug = true, feature = true, task = true, epic = true, chore = true }
graph.STATUSES = { open = true, in_progress = true, blocked = true, deferred = true, closed = true }
graph.KINDS = { blocks = true, parent = true, found_while = true, related = true }

-- The status an issue may go to from each: every one but itself, and a
-- closed issue only back to open.
graph.MOVES = {
	open = { in_progress = true, blocked = true, deferred = true, closed = true },
	in_progress = { open = true, blocked = true, deferred = true, closed = true },
	blocked = { open = true, in_progress = true, deferred = true, closed = true },
	deferred = { open = true, in_progress = true, blocked = true, closed = true },
	closed = { open = true },
}

-- Whether `issue` may go to `status`: true, or false and why not.
function graph.may_move(issue, status)
	if not graph.STATUSES[status] then
		return false, "there is no status `" .. tostring(status) .. "`"
	end
	if graph.MOVES[issue.status][status] then
		return true
	end
	if issue.status == status then
		return false, issue.id .. " is " .. status .. " already"
	end
	return false, issue.id .. " is " .. issue.status .. ": it cannot go to " .. status
end

-- The ids of `issues`, in order.
function graph.ids(issues)
	local ids = {}
	for id in pairs(issues) do
		table.insert(ids, id)
	end
	table.sort(ids)
	return ids
end

-- What `issue` links to of `kind`, in the order kept.
function graph.links(issue, kind)
	local out = {}
	for _, link in ipairs(issue.links or {}) do
		if link.kind == kind then
			table.insert(out, link.id)
		end
	end
	return out
end

-- The issues that block `id` and are not closed, in id order.
function graph.blockers(issues, id)
	local out = {}
	for _, by in ipairs(graph.links(issues[id], "blocks")) do
		local blocker = issues[by]
		if blocker and blocker.status ~= "closed" then
			table.insert(out, by)
		end
	end
	table.sort(out)
	return out
end

-- Whether `issue` waits for its date at `now`.
local function deferred(issue, now)
	return issue.status == "deferred" and not (issue.defer_until and issue.defer_until <= now)
end

-- Whether `id` is ready at `now`: open, or deferred past its date, and
-- blocked by nothing open.
function graph.is_ready(issues, id, now)
	local issue = issues[id]
	if issue.status ~= "open" and issue.status ~= "deferred" then
		return false
	end
	if deferred(issue, now) then
		return false
	end
	return #graph.blockers(issues, id) == 0
end

-- Whether `id` shows as blocked: stored so, or open with an open blocker.
function graph.is_blocked(issues, id, now)
	local issue = issues[id]
	if issue.status == "blocked" then
		return true
	end
	if issue.status == "closed" or issue.status == "in_progress" or deferred(issue, now) then
		return false
	end
	return #graph.blockers(issues, id) > 0
end

-- Sorts ids as ready issues are: by priority, then age, then id.
function graph.by_priority(issues, ids)
	table.sort(ids, function(a, b)
		local x, y = issues[a], issues[b]
		if x.priority ~= y.priority then
			return x.priority < y.priority
		end
		if x.created ~= y.created then
			return x.created < y.created
		end
		return a < b
	end)
	return ids
end

-- What matches `filter`: `{ label?, type?, priority? }`.
local function matches(issue, filter)
	if not filter then
		return true
	end
	if filter.type and issue.type ~= filter.type then
		return false
	end
	if filter.priority and issue.priority ~= filter.priority then
		return false
	end
	if filter.label then
		local found = false
		for _, label in ipairs(issue.labels or {}) do
			if label == filter.label then
				found = true
			end
		end
		if not found then
			return false
		end
	end
	return true
end
graph.matches = matches

-- The ready issues' ids at `now`, sorted, those `filter` keeps.
function graph.ready(issues, now, filter)
	local out = {}
	for id, issue in pairs(issues) do
		if graph.is_ready(issues, id, now) and matches(issue, filter) then
			table.insert(out, id)
		end
	end
	return graph.by_priority(issues, out)
end

-- The issues shown blocked at `now`, sorted as ready ones are.
function graph.blocked(issues, now, filter)
	local out = {}
	for id, issue in pairs(issues) do
		if graph.is_blocked(issues, id, now) and matches(issue, filter) then
			table.insert(out, id)
		end
	end
	return graph.by_priority(issues, out)
end

-- What `id` blocks: the issues with a `blocks` link from it, in order.
function graph.blocking(issues, id)
	local out = {}
	for other, issue in pairs(issues) do
		for _, by in ipairs(graph.links(issue, "blocks")) do
			if by == id then
				table.insert(out, other)
			end
		end
	end
	table.sort(out)
	return out
end

-- The way from `from` to `to` through `blocks` links, blocker to what
-- it blocks, as a list of ids from `from` to `to`; nil if there is none.
function graph.path(issues, from, to)
	local came = { [from] = false }
	local queue = { from }
	local at = 1
	while queue[at] do
		local id = queue[at]
		at += 1
		if id == to then
			local path = {}
			local step = id
			while step do
				table.insert(path, 1, step)
				step = came[step] or nil
			end
			return path
		end
		for _, next in ipairs(graph.blocking(issues, id)) do
			if came[next] == nil then
				came[next] = id
				table.insert(queue, next)
			end
		end
	end
	return nil
end

-- Whether `blocker` may block `id`: true, or false and the path that
-- would close a cycle, as text.
function graph.may_block(issues, blocker, id)
	if blocker == id then
		return false, id .. " cannot block itself"
	end
	local back = graph.path(issues, id, blocker)
	if back then
		table.insert(back, id)
		return false, "that closes a cycle: " .. table.concat(back, " blocks ")
	end
	return true
end

-- An epic's children, in id order.
function graph.children(issues, epic)
	local out = {}
	for id, issue in pairs(issues) do
		for _, parent in ipairs(graph.links(issue, "parent")) do
			if parent == epic then
				table.insert(out, id)
			end
		end
	end
	table.sort(out)
	return out
end

-- An epic's children closed, out of all.
function graph.progress(issues, epic)
	local children = graph.children(issues, epic)
	local done = 0
	for _, id in ipairs(children) do
		if issues[id].status == "closed" then
			done += 1
		end
	end
	return done, #children
end

-- Whether `id` may close: an epic once its children all have.
function graph.may_close(issues, id)
	local issue = issues[id]
	if issue.type ~= "epic" then
		return true
	end
	local done, all = graph.progress(issues, id)
	if done < all then
		return false, id .. " has " .. (all - done) .. " of its " .. all .. " children open"
	end
	return true
end

-- FNV-1a over `text`, as eight hex digits.
function graph.fnv(text)
	local hash = 2166136261
	for at = 1, #text do
		hash = bit32.bxor(hash, string.byte(text, at))
		-- Times 16777619, which is 2^24 + 403, kept exact in a double.
		hash = ((hash % 256) * 16777216 + hash * 403) % 4294967296
	end
	return string.format("%08x", hash)
end

-- A new issue's id: `td-` and the first four hex digits of `hash(seed)`,
-- more when those are taken, then of the seed hashed again, and past
-- that the whole digest and a counter; never one in `taken` (the ids
-- there are and were). `hash` is `graph.fnv` but in tests.
function graph.new_id(taken, seed, hash)
	hash = hash or graph.fnv
	local digest
	for round = 0, 3 do
		digest = hash(round == 0 and seed or (seed .. "#" .. round))
		for length = 4, #digest do
			local id = "td-" .. string.sub(digest, 1, length)
			if not taken[id] then
				return id
			end
		end
	end
	local n = 1
	while taken["td-" .. digest .. string.format("%x", n)] do
		n += 1
	end
	return "td-" .. digest .. string.format("%x", n)
end

-- The id of `parent`'s next child: its id, a dot, and one past the
-- highest number any child has or had.
function graph.child_id(taken, parent)
	local highest = 0
	local prefix = parent .. "."
	for id in pairs(taken) do
		if string.sub(id, 1, #prefix) == prefix then
			local n = tonumber(string.match(string.sub(id, #prefix + 1), "^(%d+)$"))
			if n and n > highest then
				highest = n
			end
		end
	end
	return prefix .. (highest + 1)
end

-- The criteria of `issue` checked, out of all.
function graph.checked(issue)
	local done = 0
	for _, criterion in ipairs(issue.criteria or {}) do
		if criterion.done then
			done += 1
		end
	end
	return done, #(issue.criteria or {})
end

return graph
