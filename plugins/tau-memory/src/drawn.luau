-- What tau-memory's tests draw, and a folder they keep notes in: notes
-- valid by construction, ids from a small pool so notes collide and
-- link to one another, and files in memory, as `ctx.files` reads and
-- writes them. Only tests use it.
local note = require("note")

local M = {}

-- Files in memory, by path: what `ctx.files` reads, lists and writes.
function M.files(start)
	local held = start or {}
	local files = { held = held }
	function files.read(path)
		local found = held[path]
		if found == nil then
			error("Cannot read " .. path .. ": No such file or directory (os error 2)", 0)
		end
		return found
	end
	function files.list(path)
		local prefix = path .. "/"
		local names, seen = {}, {}
		for at in pairs(held) do
			if string.sub(at, 1, #prefix) == prefix then
				local rest = string.sub(at, #prefix + 1)
				local slash = string.find(rest, "/", 1, true)
				local name = slash and string.sub(rest, 1, slash - 1) or rest
				if not seen[name] then
					seen[name] = true
					table.insert(names, { name = name, dir = slash ~= nil, size = 0 })
				end
			end
		end
		if #names == 0 then
			error("Cannot list " .. path .. ": No such file or directory (os error 2)", 0)
		end
		table.sort(names, function(a, b)
			return a.name < b.name
		end)
		return names
	end
	function files.write(path, text)
		held[path] = text
	end
	function files.remove(path)
		if held[path] == nil then
			error("Cannot remove " .. path .. ": No such file or directory (os error 2)", 0)
		end
		held[path] = nil
	end
	function files.mkdir(_path) end
	function files.roots()
		return {}
	end
	return files
end

-- A note id.
function M.id(g)
	return g.from_regex("^[a-z0-9][a-z0-9-]{0,20}$")
end

-- One line of any text, not blank.
function M.line(g)
	return g.text({ max = 40 }):map(function(s)
		return "x" .. string.gsub(s, "[\n\r]", "")
	end)
end

local function list(draw, g, max)
	local out = {}
	for _ = 1, draw(g.integers({ min = 0, max = max })) do
		table.insert(out, draw(M.line(g)))
	end
	return out
end

local function optional(draw, g, inner)
	if draw(g.booleans()) then
		return draw(inner)
	end
	return nil
end

-- Any note, every field drawn.
function M.note(g)
	return g.composite(function(draw)
		local links = {}
		for _ = 1, draw(g.integers({ min = 0, max = 3 })) do
			table.insert(links, {
				to = draw(M.id(g)),
				type = draw(g.sampled_from(note.LINKS)),
				why = optional(draw, g, M.line(g)),
			})
		end
		local time = g.integers({ min = 0, max = 2 ^ 50 })
		local source = {
			by = draw(g.sampled_from(note.BY)),
			run = optional(draw, g, M.line(g)),
			turn = optional(draw, g, g.integers({ min = 0, max = 4294967295 })),
			commit = optional(draw, g, M.line(g)),
			files = list(draw, g, 3),
		}
		if draw(g.booleans()) then
			source.imported = {
				from = draw(g.sampled_from(note.TOOLS)),
				path = draw(M.line(g)),
				session = optional(draw, g, M.line(g)),
			}
		end
		return {
			id = draw(M.id(g)),
			title = draw(M.line(g)),
			description = draw(M.line(g)),
			type = draw(g.sampled_from(note.TYPES)),
			tags = list(draw, g, 3),
			created = draw(time),
			updated = draw(time),
			valid_from = draw(time),
			valid_to = optional(draw, g, time),
			stale = optional(draw, g, M.line(g)),
			source = source,
			links = links,
			-- Any body, fence lines included: only the first closes the
			-- front matter.
			body = draw(g.one_of({ g.text({ max = 200 }), g.just("+++\n+++\nstill body\n") })),
		}
	end)
end

-- The ids `pool_id` draws.
M.IDS = { "a", "b", "c", "d", "e" }

function M.pool_id(g)
	return g.sampled_from(M.IDS)
end

-- A current note from the pool, linking within it, with some bare
-- `[[id]]` links in its body; written before the tests' clock starts,
-- at 1,000, or just after.
function M.pool_note(g)
	return g.composite(function(draw)
		local n = draw(M.note(g))
		n.id = draw(M.pool_id(g))
		n.valid_to = nil
		n.stale = nil
		n.created = draw(g.integers({ min = 0, max = 1100 }))
		n.updated = draw(g.integers({ min = 0, max = 1100 }))
		n.links = {}
		for _ = 1, draw(g.integers({ min = 0, max = 3 })) do
			table.insert(n.links, {
				to = draw(M.pool_id(g)),
				type = draw(g.sampled_from({ "relates", "refines", "contradicts", "derived_from" })),
			})
		end
		n.source.files = {}
		for _, file in ipairs({ "src/a.rs", "src/b.rs" }) do
			if draw(g.booleans()) then
				table.insert(n.source.files, file)
			end
		end
		if draw(g.booleans()) then
			table.insert(n.links, { to = draw(g.sampled_from({ "src/a.rs", "src/b.rs" })), type = "about" })
		end
		local body = {}
		for _ = 1, draw(g.integers({ min = 0, max = 2 })) do
			table.insert(body, "see [[" .. draw(M.pool_id(g)) .. "]]\n")
		end
		n.body = table.concat(body)
		return n
	end)
end

return M
