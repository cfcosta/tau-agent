-- The issue graph's laws (ADR 0048, "1. The issue graph"): the graph and
-- beads modules, called with drawn graphs and held to a reference model
-- that answers each question the slow, plain way (ADR 0049).
local tau = require("tau")
local t = tau.test
local g = t.gen
local graph = require("graph")
local beads = require("beads")

local STATUSES = { "open", "in_progress", "blocked", "deferred", "closed" }
local KINDS = { "blocks", "parent", "found_while", "related" }
local TYPES = { "bug", "feature", "task", "epic", "chore" }

-- The reference model, over issues as the plugin keeps them.
local model = {}

-- Whether `blocker` blocks `id`: a `blocks` link on `id` naming it.
function model.blocks(issues, blocker, id)
	for _, link in ipairs(issues[id].links) do
		if link.id == blocker and link.kind == "blocks" then
			return true
		end
	end
	return false
end

local function sorted_ids(issues)
	local ids = {}
	for id in pairs(issues) do
		table.insert(ids, id)
	end
	table.sort(ids)
	return ids
end

-- The open issues that block `id`.
function model.open_blockers(issues, id)
	local out = {}
	for _, other in ipairs(sorted_ids(issues)) do
		if issues[other].status ~= "closed" and model.blocks(issues, other, id) then
			table.insert(out, other)
		end
	end
	return out
end

local function waits(issue, now)
	return issue.status == "deferred" and (issue.defer_until == nil or issue.defer_until > now)
end

-- Ready at `now`: open, or deferred past its date, nothing open
-- blocking it; by priority, then age, then id.
function model.ready(issues, now)
	local ready = {}
	for _, id in ipairs(sorted_ids(issues)) do
		local issue = issues[id]
		if
			(issue.status == "open" or issue.status == "deferred")
			and not waits(issue, now)
			and #model.open_blockers(issues, id) == 0
		then
			table.insert(ready, issue)
		end
	end
	table.sort(ready, function(a, b)
		if a.priority ~= b.priority then
			return a.priority < b.priority
		end
		if a.created ~= b.created then
			return a.created < b.created
		end
		return a.id < b.id
	end)
	local out = {}
	for _, issue in ipairs(ready) do
		table.insert(out, issue.id)
	end
	return out
end

-- Shown blocked at `now`: stored so, or open with an open blocker.
function model.is_blocked(issues, id, now)
	local issue = issues[id]
	if issue.status == "blocked" then
		return true
	end
	if issue.status == "closed" or issue.status == "in_progress" or waits(issue, now) then
		return false
	end
	return #model.open_blockers(issues, id) > 0
end

-- Whether some way leads from `from` to `to` through `blocks` links,
-- blocker to blocked: every path walked.
function model.reaches(issues, from, to)
	local seen = {}
	local function walk(at)
		if at == to then
			return true
		end
		if seen[at] then
			return false
		end
		seen[at] = true
		for _, next in ipairs(sorted_ids(issues)) do
			if model.blocks(issues, at, next) and walk(next) then
				seen[at] = nil
				return true
			end
		end
		seen[at] = nil
		return false
	end
	return walk(from)
end

-- An epic's children, and how many are closed.
function model.progress(issues, epic)
	local done, all = 0, 0
	for _, issue in pairs(issues) do
		for _, link in ipairs(issue.links) do
			if link.id == epic and link.kind == "parent" then
				all += 1
				if issue.status == "closed" then
					done += 1
				end
				break
			end
		end
	end
	return done, all
end

-- A graph of one to `most` issues: priorities, ages that tie, statuses,
-- dates past and to come, epics, and links of every kind between them,
-- cycles of `blocks` among them.
local function issues(most)
	return g.composite(function(draw)
		local n = draw(g.integers({ min = 1, max = most }))
		local out, ids = {}, {}
		for at = 0, n - 1 do
			local id = string.format("td-%04x", at)
			local kind = draw(g.sampled_from(TYPES))
			local status = draw(g.sampled_from(STATUSES))
			local deferred = status == "deferred" and draw(g.booleans())
			local priority = draw(g.integers({ min = 0, max = 4 }))
			local created = draw(g.integers({ min = 0, max = 5 }))
			local issue = {
				id = id,
				title = "issue " .. id,
				description = "",
				criteria = {},
				type = kind,
				priority = priority,
				status = status,
				labels = {},
				comments = {},
				links = {},
				created = created,
				updated = created,
			}
			if deferred then
				issue.defer_until = draw(g.integers({ min = 0, max = 10 }))
			end
			if status == "closed" then
				issue.closed = created + 1
				issue.close_reason = "done"
			end
			out[id] = issue
			table.insert(ids, id)
		end
		for _ = 1, draw(g.integers({ min = 0, max = n * 2 })) do
			local from = draw(g.sampled_from(ids))
			local on = draw(g.sampled_from(ids))
			local kind = draw(g.sampled_from(KINDS))
			if from ~= on and not model.has_link(out[from], on, kind) then
				table.insert(out[from].links, { id = on, kind = kind })
			end
		end
		return out
	end)
end

function model.has_link(issue, on, kind)
	for _, link in ipairs(issue.links) do
		if link.id == on and link.kind == kind then
			return true
		end
	end
	return false
end

t.property("ready and blocked are the model's", function(draw)
	local all = draw(issues(8))
	local now = draw(g.integers({ min = 0, max = 10 }))
	t.equal(graph.ready(all, now), model.ready(all, now), "ready")
	for _, id in ipairs(sorted_ids(all)) do
		t.equal(graph.is_blocked(all, id, now), model.is_blocked(all, id, now), id .. " blocked")
	end
end, { cases = 200 })

-- A `blocks` link is refused exactly when a way already leads back, and
-- the refusal names a real way: each step a link there is, ending in the
-- link asked for.
t.property("a blocks cycle is refused with its path", function(draw)
	local all = draw(issues(7))
	local ids = sorted_ids(all)
	local blocker = draw(g.sampled_from(ids))
	local id = draw(g.sampled_from(ids))
	-- Sometimes a long way round: a chain of three to six, each blocking
	-- the next, and the link asked for would close it.
	if #ids >= 3 and draw(g.booleans()) then
		local length = draw(g.integers({ min = 3, max = math.min(#ids, 6) }))
		for at = 1, length - 1 do
			local blocked = all[ids[at + 1]]
			if not model.has_link(blocked, ids[at], "blocks") then
				table.insert(blocked.links, { id = ids[at], kind = "blocks" })
			end
		end
		id = ids[1]
		blocker = ids[length]
	end
	local ok, why = graph.may_block(all, blocker, id)
	local refused = blocker == id or model.reaches(all, id, blocker)
	t.equal(ok, not refused, "refused")
	if refused and blocker ~= id then
		local path = string.match(why, "^that closes a cycle: (.*)$")
		t.equal(path ~= nil, true, why)
		local steps = string.split(path, " blocks ")
		t.equal(steps[1], id, why)
		t.equal(steps[#steps], id, why)
		t.equal(steps[#steps - 1], blocker, why)
		for at = 1, #steps - 2 do
			local a, b = steps[at], steps[at + 1]
			t.equal(b == id or model.has_link(all[b], a, "blocks"), true, a .. " blocks " .. b .. " is no link: " .. why)
		end
	end
end, { cases = 200 })

-- An epic's progress counts exactly its children, and it may close once
-- they all have, not before.
t.property("an epic counts its children", function(draw)
	local all = draw(issues(8))
	for _, id in ipairs(sorted_ids(all)) do
		local done, every = graph.progress(all, id)
		local want_done, want_all = model.progress(all, id)
		t.equal(done, want_done, id .. " done")
		t.equal(every, want_all, id .. " children")
		local may = all[id].type ~= "epic" or want_done == want_all
		t.equal(graph.may_close(all, id), may, id .. " may close")
	end
end, { cases = 150 })

-- Ids are unique across any creates, with hashes forced to collide; a
-- child's is its epic's and one past the highest number used.
t.property("ids never repeat", function(draw)
	local digests = draw(g.lists(g.sampled_from({ "aaaaaaaa", "aaaabbbb", "aaaabbbc", "12345678" }), { min = 1, max = 12 }))
	local taken, made = {}, {}
	for n, digest in ipairs(digests) do
		local id = graph.new_id(taken, "seed" .. n, function()
			return digest
		end)
		t.equal(taken[id], nil, id .. " again")
		t.equal(string.sub(id, 1, 3) == "td-" and #id >= 7, true, id)
		taken[id] = true
		table.insert(made, id)
	end
	local epic = made[1]
	for n = 1, 3 do
		local child = graph.child_id(taken, epic)
		t.equal(child, epic .. "." .. n)
		taken[child] = true
	end
end, { cases = 150 })

-- Every status may go to every other but itself, and a closed one only
-- back to open; a move refused names the status the issue had.
t.case("every move is in the table", function()
	for _, from in ipairs(STATUSES) do
		for _, to in ipairs(STATUSES) do
			local ok, why = graph.may_move({ id = "td-1", status = from }, to)
			local allowed = from ~= to and (from ~= "closed" or to == "open")
			t.equal(ok, allowed, from .. " to " .. to)
			if not allowed then
				t.equal(string.find(why, from, 1, true) ~= nil, true, from .. " to " .. to .. ": " .. why)
			end
		end
	end
end)

-- An issue with every field: criteria, comments, labels, dates and a
-- stored blocked status among them.
local words = g.lists(g.sampled_from({ "fix", "the", '"sync"', "é", "\n", "a\\b" }), { max = 4 })

local function full(all)
	return g.composite(function(draw)
		local out = {}
		for _, id in ipairs(sorted_ids(all)) do
			local issue = table.clone(all[id])
			issue.title = "t" .. id .. " " .. table.concat(draw(words), " ")
			issue.description = table.concat(draw(words), " ")
			issue.criteria = {}
			for n, done in ipairs(draw(g.lists(g.booleans(), { max = 3 }))) do
				table.insert(issue.criteria, { text = "criterion " .. (n - 1), done = done })
			end
			issue.comments = {}
			for n = 0, draw(g.integers({ min = 0, max = 2 })) - 1 do
				table.insert(issue.comments, { at = 1793700000 + n, by = "r1", text = "said " .. n .. "\nmore" })
			end
			issue.labels = draw(g.booleans()) and { "phone", "sync" } or {}
			if issue.status == "blocked" and draw(g.booleans()) then
				issue.blocked = "the vendor"
			end
			-- A Beads assignee, which claims nothing: a claim is a run's.
			if draw(g.booleans()) then
				issue.assignee = "alice"
			end
			issue.created += 1793000000
			issue.updated += 1793000000
			out[id] = issue
		end
		return out
	end)
end

-- Read back, an export is every field it had; written again, the same
-- bytes, sorted by id.
t.property("an export round trips", function(draw)
	local all = draw(full(draw(issues(6))))
	local text = beads.export(all)
	local back = beads.import(text)
	t.equal(beads.export(back), text, "the same bytes")
	t.equal(back, all, "every field")
	local lines = {}
	for line in string.gmatch(text, "[^\n]+") do
		table.insert(lines, line)
	end
	local sorted = table.clone(lines)
	table.sort(sorted)
	t.equal(lines, sorted, "sorted by id")
end, { cases = 150 })
