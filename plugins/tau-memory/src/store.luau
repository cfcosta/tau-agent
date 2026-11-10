-- A scope's notes: a folder with one file per note at `<type>/<id>.md`,
-- and `.history/<id>/<n>.md` holding every version a write replaced,
-- so any change can be looked at and undone. Read and written through
-- `ctx.files`.
--
-- The files are the truth: `open` reads them all, and anything derived
-- from them (links, backlinks, search) is rebuilt from them. A file
-- that does not read as a note is reported, not dropped.
local note = require("note")

local M = {}

local HISTORY = ".history"

-- `s` quoted as Rust's `{:?}` quotes it, for messages.
function M.quoted(s)
	local out = string.gsub(s, '[%c"\\]', function(c)
		if c == '"' then
			return '\\"'
		elseif c == "\\" then
			return "\\\\"
		elseif c == "\n" then
			return "\\n"
		elseif c == "\r" then
			return "\\r"
		elseif c == "\t" then
			return "\\t"
		elseif c == "\0" then
			return "\\0"
		end
		return string.format("\\u{%x}", string.byte(c))
	end)
	return '"' .. out .. '"'
end

-- Why a store operation failed: `kind` is `"exists"`, `"missing"`,
-- `"note"` (it would not read back) or `"io"`.
local function failure(kind, message)
	return { kind = kind, message = message }
end
M.failure = failure

local Notes = {}
Notes.__index = Notes

-- What a file is called, its stem and extension, as Rust's `Path` has
-- them: `.md` alone has no extension.
local function split_name(name)
	local stem, ext = string.match(name, "^(.+)%.([^.]*)$")
	if stem == nil then
		return name, nil
	end
	return stem, ext
end

-- A folder's entries, or none when it is not there.
local function entries(files, dir)
	local ok, found = pcall(files.list, dir)
	if not ok then
		return {}
	end
	return found
end

-- Reads every note under `dir`, through `files` (`ctx.files`).
function M.open(files, dir)
	local self = setmetatable({ files = files, dir = dir, notes = {}, unreadable = {} }, Notes)
	for _, kind in ipairs(note.TYPES) do
		local folder = dir .. "/" .. kind
		local names = {}
		for _, entry in ipairs(entries(files, folder)) do
			local _, ext = split_name(entry.name)
			if ext == "md" then
				table.insert(names, entry.name)
			end
		end
		table.sort(names)
		for _, name in ipairs(names) do
			local path = folder .. "/" .. name
			local found, why = self:read_note(path, name)
			if found == nil then
				table.insert(self.unreadable, { path = path, why = why })
			elseif found.type ~= kind then
				table.insert(self.unreadable, { path = path, why = "a " .. found.type .. " note in the " .. kind .. " folder" })
			elseif self.notes[found.id] then
				table.insert(self.unreadable, { path = path, why = "a second note called " .. M.quoted(found.id) })
			else
				self.notes[found.id] = found
			end
		end
	end
	return self
end

function Notes:read_note(path, name)
	local ok, file = pcall(self.files.read, path)
	if not ok then
		return nil, tostring(file)
	end
	local found, why = note.parse(file)
	if found == nil then
		return nil, why
	end
	local stem = split_name(name)
	if stem ~= found.id or not note.is_id(found.id) then
		return nil, "the file is not named after its id " .. M.quoted(found.id)
	end
	return found
end

function Notes:get(id)
	return self.notes[id]
end

-- Every note's id, in order.
function Notes:ids()
	local ids = {}
	for id in pairs(self.notes) do
		table.insert(ids, id)
	end
	table.sort(ids)
	return ids
end

-- Every note, by id.
function Notes:list()
	local out = {}
	for _, id in ipairs(self:ids()) do
		table.insert(out, self.notes[id])
	end
	return out
end

function Notes:len()
	local n = 0
	for _ in pairs(self.notes) do
		n += 1
	end
	return n
end

-- Where `n`'s file lives.
function Notes:path_of(n)
	return self.dir .. "/" .. n.type .. "/" .. n.id .. ".md"
end

local function write(files, path, contents)
	local ok, why = pcall(files.write, path, contents)
	if not ok then
		return failure("io", "notes: " .. tostring(why))
	end
	return nil
end

function Notes:put(n)
	local why = note.validate(n)
	if why then
		return nil, failure("note", why)
	end
	local failed = write(self.files, self:path_of(n), note.render(n))
	if failed then
		return nil, failed
	end
	self.notes[n.id] = n
	return true
end

-- Writes a new note. Refused when its id is taken.
function Notes:create(n)
	if self.notes[n.id] then
		return nil, failure("exists", "a note is already called " .. M.quoted(n.id))
	end
	return self:put(n)
end

-- The numbers of the versions kept of `id`, as files are named.
function Notes:versions(id)
	local found = {}
	for _, entry in ipairs(entries(self.files, self.dir .. "/" .. HISTORY .. "/" .. id)) do
		local stem = split_name(entry.name)
		local n = string.match(stem, "^%+?(%d+)$")
		if n and #n <= 19 then
			table.insert(found, { n = tonumber(n), name = entry.name })
		end
	end
	return found
end

-- Keeps `n` as it is now in the history, as its next version.
function Notes:snapshot(n)
	local next = 0
	for _, version in ipairs(self:versions(n.id)) do
		next = math.max(next, version.n + 1)
	end
	local path = self.dir .. "/" .. HISTORY .. "/" .. n.id .. "/" .. string.format("%d", next) .. ".md"
	return write(self.files, path, note.render(n))
end

-- Writes a new version of an existing note, keeping the one it replaces
-- in the history. A change of type moves the file.
function Notes:update(n)
	local old = self.notes[n.id]
	if old == nil then
		return nil, failure("missing", "no note is called " .. M.quoted(n.id))
	end
	local failed = self:snapshot(old)
	if failed then
		return nil, failed
	end
	if old.type ~= n.type then
		local ok, why = pcall(self.files.remove, self:path_of(old))
		if not ok then
			return nil, failure("io", "notes: " .. tostring(why))
		end
	end
	return self:put(n)
end

-- Writes `new`, which replaces `old`: `new` gets a `supersedes` link to
-- it, and `old` stays with `valid_to` set to `now`.
function Notes:supersede(old, new, now)
	local previous = self.notes[old]
	if previous == nil then
		return nil, failure("missing", "no note is called " .. M.quoted(old))
	end
	local why = note.validate(new)
	if why then
		return nil, failure("note", why)
	end
	if new.id == old or self.notes[new.id] then
		return nil, failure("exists", "a note is already called " .. M.quoted(new.id))
	end
	previous = note.copy(previous)
	local linked = false
	for _, link in ipairs(new.links) do
		if link.to == old and link.type == "supersedes" then
			linked = true
		end
	end
	if not linked then
		table.insert(new.links, { to = old, type = "supersedes" })
	end
	previous.valid_to = now
	previous.updated = now
	local ok, failed = self:create(new)
	if not ok then
		return nil, failed
	end
	return self:update(previous)
end

-- Marks each note about one of `paths` as possibly stale: an `about`
-- link to it, or a source file that is it. Only notes last written at
-- or before `written_by` are marked: a change that began before a note
-- was written cannot make it stale. Superseded notes are left alone.
-- Returns the ids marked.
function Notes:mark_stale(paths, why, written_by, now)
	local touched = {}
	for _, n in ipairs(self:list()) do
		if n.valid_to == nil and n.stale == nil and n.updated <= written_by then
			local about = false
			for _, link in ipairs(n.links) do
				if link.type == "about" and table.find(paths, link.to) then
					about = true
				end
			end
			for _, file in ipairs(n.source.files or {}) do
				if table.find(paths, file) then
					about = true
				end
			end
			if about then
				table.insert(touched, n.id)
			end
		end
	end
	for _, id in ipairs(touched) do
		local marked = note.copy(self.notes[id])
		marked.stale = why
		marked.updated = now
		local ok, failed = self:update(marked)
		if not ok then
			return nil, failed
		end
	end
	return touched
end

-- The versions a note had before its latest write, oldest first.
function Notes:history(id)
	local versions = self:versions(id)
	table.sort(versions, function(a, b)
		return a.n < b.n
	end)
	local out = {}
	for _, version in ipairs(versions) do
		local ok, file = pcall(self.files.read, self.dir .. "/" .. HISTORY .. "/" .. id .. "/" .. version.name)
		if ok then
			local found = note.parse(file)
			if found then
				table.insert(out, found)
			end
		end
	end
	return out
end

-- Makes the note as it was at `version` (from 1, into `history`) the
-- latest again; the current one goes into the history, so this can be
-- undone too.
function Notes:revert(id, version)
	local old = self:history(id)[version]
	if old == nil then
		return nil, failure("missing", "no note is called " .. M.quoted(id .. " version " .. tostring(version - 1)))
	end
	return self:update(old)
end

-- Links into the note `id`, as `{ from, link }`.
function Notes:backlinks(id)
	local found = {}
	for _, n in ipairs(self:list()) do
		for _, link in ipairs(note.all_links(n)) do
			if link.to == id and link.type ~= "about" then
				table.insert(found, { from = n.id, link = link })
			end
		end
	end
	return found
end

-- The notes linked to or from `id`, each once: what one hop along links
-- reaches.
function Notes:neighbours(id)
	local found = {}
	local function add(other)
		if other ~= id and self.notes[other] and not table.find(found, other) then
			table.insert(found, other)
		end
	end
	local n = self.notes[id]
	if n then
		for _, link in ipairs(note.all_links(n)) do
			if link.type ~= "about" then
				add(link.to)
			end
		end
	end
	for _, back in ipairs(self:backlinks(id)) do
		add(back.from)
	end
	return found
end

-- Links to notes that do not exist yet, as `{ from, to }`. They resolve
-- once a note with that id is written.
function Notes:dangling()
	local found = {}
	for _, n in ipairs(self:list()) do
		for _, link in ipairs(note.all_links(n)) do
			if link.type ~= "about" and not self.notes[link.to] then
				table.insert(found, { from = n.id, to = link.to })
			end
		end
	end
	return found
end

return M
