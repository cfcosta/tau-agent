-- The store against a model: a map of notes, with every replaced
-- version kept. Reopening the folder reads back what the model holds.
local tau = require("tau")
local t = tau.test
local g = t.gen
local note = require("note")
local store = require("store")
local drawn = require("drawn")

local DIR = "/notes"

local function copy(n)
	return note.copy(n)
end

-- Writes `n` over the model's note of its id, keeping that in history.
local function replace(model, n)
	local old = model.notes[n.id]
	assert(old, "replaces a note")
	model.history[n.id] = model.history[n.id] or {}
	table.insert(model.history[n.id], old)
	model.notes[n.id] = n
end

-- Backlinks the plain way: every link, from every note, by id.
local function backlinks(model, id)
	local ids = {}
	for other in pairs(model.notes) do
		table.insert(ids, other)
	end
	table.sort(ids)
	local found = {}
	for _, from in ipairs(ids) do
		for _, link in ipairs(note.all_links(model.notes[from])) do
			if link.to == id and link.type ~= "about" then
				table.insert(found, { from = from, link = link })
			end
		end
	end
	return found
end

t.machine("the store matches its model", {
	init = function()
		local files = drawn.files()
		return { files = files, store = store.open(files, DIR), notes = {}, history = {}, now = 1000 }
	end,
	rules = {
		create = function(m, draw)
			m.now += 1
			local n = draw(drawn.pool_note(g))
			local taken = m.notes[n.id] ~= nil
			local ok, err = m.store:create(copy(n))
			if ok then
				t.equal(taken, false, "created a new id")
				m.notes[n.id] = n
			else
				t.equal(err.kind, "exists", err.message)
				t.equal(taken, true, "refused a taken id")
			end
		end,
		update = function(m, draw)
			m.now += 1
			local n = draw(drawn.pool_note(g))
			n.updated = m.now
			local known = m.notes[n.id] ~= nil
			local ok, err = m.store:update(copy(n))
			if ok then
				t.equal(known, true, "updated a known note")
				replace(m, n)
			else
				t.equal(err.kind, "missing", err.message)
				t.equal(known, false, "refused an unknown note")
			end
		end,
		supersede = function(m, draw)
			m.now += 1
			local old = draw(drawn.pool_id(g))
			local new = draw(drawn.pool_note(g))
			local known = m.notes[old] ~= nil
			local free = new.id ~= old and m.notes[new.id] == nil
			local kept = {}
			for _, link in ipairs(new.links) do
				if link.to ~= old then
					table.insert(kept, link)
				end
			end
			new.links = kept
			local ok, err = m.store:supersede(old, copy(new), m.now)
			if ok then
				t.equal(known and free, true, "superseded")
				table.insert(new.links, { to = old, type = "supersedes" })
				m.notes[new.id] = new
				local previous = copy(m.notes[old])
				previous.valid_to = m.now
				previous.updated = m.now
				replace(m, previous)
			elseif err.kind == "missing" then
				t.equal(known, false, err.message)
			else
				t.equal(err.kind, "exists", err.message)
				t.equal(known and not free, true, err.message)
			end
		end,
		-- Stale marks follow the files a note came from or is about, and
		-- only reach notes written before the change began.
		mark_stale = function(m, draw)
			m.now += 1
			local paths = {}
			for _, path in ipairs({ "src/a.rs", "src/b.rs" }) do
				if draw(g.booleans()) then
					table.insert(paths, path)
				end
			end
			local written_by = draw(g.integers({ min = 1000, max = m.now }))
			local ids = {}
			for id in pairs(m.notes) do
				table.insert(ids, id)
			end
			table.sort(ids)
			local want = {}
			for _, id in ipairs(ids) do
				local n = m.notes[id]
				local about = false
				for _, file in ipairs(n.source.files) do
					about = about or table.find(paths, file) ~= nil
				end
				for _, link in ipairs(n.links) do
					about = about or (link.type == "about" and table.find(paths, link.to) ~= nil)
				end
				if n.valid_to == nil and n.stale == nil and n.updated <= written_by and about then
					table.insert(want, id)
				end
			end
			local marked = m.store:mark_stale(paths, "src changed", written_by, m.now)
			t.equal(marked, want, "marked")
			for _, id in ipairs(marked) do
				local n = copy(m.notes[id])
				n.stale = "src changed"
				n.updated = m.now
				replace(m, n)
			end
		end,
		-- Any version the model kept comes back, and the one it replaces
		-- goes into history in turn.
		revert = function(m, draw)
			m.now += 1
			local id = draw(drawn.pool_id(g))
			local versions = m.history[id] or {}
			if #versions == 0 then
				t.equal(m.store:revert(id, 1), nil, "nothing to revert to")
			else
				local at = draw(g.integers({ min = 1, max = #versions }))
				t.equal(m.store:revert(id, at), true, "reverted")
				replace(m, copy(versions[at]))
			end
			-- Past the last version is never one.
			t.equal(m.store:revert(id, #versions + 2), nil, "past the last version")
		end,
		reopen = function(m)
			m.store = store.open(m.files, DIR)
			t.equal(m.store.unreadable, {}, "every file reads")
		end,
	},
	-- The store holds the model's notes, every version it kept, and the
	-- model's links.
	invariant = function(m)
		local held = {}
		for _, n in ipairs(m.store:list()) do
			held[n.id] = n
		end
		t.equal(held, m.notes, "the notes")
		for _, id in ipairs(drawn.IDS) do
			t.equal(m.store:history(id), m.history[id] or {}, "history of " .. id)
			t.equal(m.store:backlinks(id), backlinks(m, id), "backlinks of " .. id)
			-- One hop reaches exactly the notes linked either way.
			local hop = m.store:neighbours(id)
			table.sort(hop)
			local want, seen = {}, {}
			local function add(other)
				if other ~= id and m.notes[other] and not seen[other] then
					seen[other] = true
					table.insert(want, other)
				end
			end
			for _, back in ipairs(backlinks(m, id)) do
				add(back.from)
			end
			if m.notes[id] then
				for _, link in ipairs(note.all_links(m.notes[id])) do
					add(link.to)
				end
			end
			table.sort(want)
			t.equal(hop, want, "neighbours of " .. id)
		end
	end,
	steps = 25,
}, { cases = 60 })

-- A file the store cannot read is reported with its path, and the rest
-- still load: notes on either side of it in its folder, and in another
-- folder.
t.case("a broken file is reported, not dropped", function()
	local files = drawn.files()
	local notes = store.open(files, DIR)
	for _, made in ipairs({ { "a-first", "fact" }, { "retry-after", "fact" }, { "lanes", "gotcha" } }) do
		local ok = notes:create({
			id = made[1],
			title = "about " .. made[1],
			description = "a note beside a broken one",
			type = made[2],
			tags = {},
			created = 1,
			updated = 1,
			valid_from = 1,
			source = note.source("agent"),
			links = {},
			body = "It reads.",
		})
		t.equal(ok, true)
	end
	files.write(DIR .. "/fact/broken.md", "no front matter")
	notes = store.open(files, DIR)
	t.equal(notes:ids(), { "a-first", "lanes", "retry-after" })
	t.equal(#notes.unreadable, 1)
	t.equal(notes.unreadable[1].path, DIR .. "/fact/broken.md")
end)

-- Through a run's `ctx.files`, as the plugin reads and writes them: the
-- note lands in its type's folder, and its old version in the history.
t.case("the store writes through ctx.files", function()
	local run = t.run({ roots = { ["repository/tau-memory"] = "/r/tau-memory" } })
	run:tool("memory_write", { type = "fact", title = "Retry reads the header", description = "d", body = "first" })
	run:tool("memory_write", { type = "fact", title = "Retry reads the header", description = "d", body = "second", id = "retry-reads-the-header" })
	local path = "/r/tau-memory/fact/retry-reads-the-header.md"
	t.equal(note.parse(run.files[path]).body, "second")
	t.equal(note.parse(run.files["/r/tau-memory/.history/retry-reads-the-header/0.md"]).body, "first")
end)
