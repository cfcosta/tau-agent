-- A note: one idea, as a Markdown file with TOML front matter between
-- `+++` lines (`docs/reference/plugins.md`, `tau-memory`).
--
--   +++
--   id = "retry-after-http-date"
--   title = "retry-after can be an HTTP date"
--   description = "Parse both forms; a bad one falls back to backoff"
--   type = "gotcha"
--   created = 1790000000000
--   updated = 1790000000000
--   valid_from = 1790000000000
--
--   [source]
--   by = "agent"
--   run = "01j…"
--
--   [[links]]
--   to = "retry-policy"
--   type = "refines"
--   +++
--   The body, in full prose: exact versions, flags, paths and errors.
--
-- A note is a table of those fields, `body` the text after the front
-- matter, `tags` and `links` lists, `source.files` a list; times are
-- milliseconds since the Unix epoch. A bare `[[id]]` in the body is a
-- `relates` link, as if it were listed.
local text = require("text")
local toml = require("toml")

local M = {}

-- What a note holds: a closed set, also each note's folder.
M.TYPES = { "fact", "convention", "decision", "gotcha", "procedure", "case", "preference", "index" }

-- How one note relates to another; `about` points at a path.
M.LINKS = { "relates", "refines", "supersedes", "contradicts", "derived_from", "about" }

-- Who a claim comes from.
M.BY = { "user", "agent", "inferred" }

-- The coding agents a note may be imported from.
M.TOOLS = { "claude_code", "codex" }

local function member(list, value)
	return table.find(list, value) ~= nil
end

function M.is_type(name)
	return member(M.TYPES, name)
end

function M.is_link(name)
	return member(M.LINKS, name)
end

-- Whether `id` can name a note: `a-z`, `0-9` and `-`, 1 to 64 long.
function M.is_id(id)
	return type(id) == "string" and #id >= 1 and #id <= 64 and string.match(id, "^[a-z0-9%-]+$") ~= nil
end

-- An id made from a title: lowercase ASCII words joined by `-`, at most
-- 64 long; `note` when nothing in the title can be kept.
function M.slug(title)
	local id = ""
	for word in string.gmatch(title, "[%w]+") do
		-- `%w` is ASCII letters and digits only.
		word = string.lower(word)
		local sep = id == "" and 0 or 1
		if #id + sep + #word > 64 then
			break
		end
		if sep == 1 then
			id ..= "-"
		end
		id ..= word
	end
	if id == "" then
		return "note"
	end
	return id
end

-- A new source: who it is by, nothing else.
function M.source(by)
	return { by = by, files = {} }
end

local function breaks(s)
	return string.find(s, "[\n\r]") ~= nil
end

local function one_line(s)
	return not text.blank(s) and not breaks(s)
end

-- Why `note` could not be written so that it reads back the same, or nil.
function M.validate(note)
	if not M.is_id(note.id) then
		return string.format("%q is not a note id: use a-z, 0-9 and -, 1 to 64 long", note.id)
	end
	if not one_line(note.title) then
		return "the title must be one line, and not empty"
	end
	if not one_line(note.description) then
		return "the description must be one line, and not empty"
	end
	for _, link in ipairs(note.links or {}) do
		if not one_line(link.to) then
			return "the link target must be one line, and not empty"
		end
	end
	-- Everything else in the front matter is one line too, so no value
	-- can hold a line that closes it.
	local source = note.source
	local fields = {}
	for _, tag in ipairs(note.tags or {}) do
		table.insert(fields, tag)
	end
	for _, link in ipairs(note.links or {}) do
		table.insert(fields, link.why)
	end
	for _, file in ipairs(source.files or {}) do
		table.insert(fields, file)
	end
	table.insert(fields, source.run)
	table.insert(fields, source.commit)
	table.insert(fields, note.stale)
	if source.imported then
		table.insert(fields, source.imported.path)
		table.insert(fields, source.imported.session)
	end
	for _, field in ipairs(fields) do
		if breaks(field) then
			return "the front matter must be one line, and not empty"
		end
	end
	return nil
end

-- The note as its file holds it, front matter as the toml crate wrote it.
function M.render(note)
	local lines = {}
	local function put(key, value)
		table.insert(lines, key .. " = " .. value)
	end
	local s, n = toml.string, toml.integer
	put("id", s(note.id))
	put("title", s(note.title))
	put("description", s(note.description))
	put("type", s(note.type))
	if note.tags and #note.tags > 0 then
		put("tags", toml.strings(note.tags))
	end
	put("created", n(note.created))
	put("updated", n(note.updated))
	put("valid_from", n(note.valid_from))
	if note.valid_to ~= nil then
		put("valid_to", n(note.valid_to))
	end
	if note.stale ~= nil then
		put("stale", s(note.stale))
	end
	local source = note.source
	table.insert(lines, "")
	table.insert(lines, "[source]")
	put("by", s(source.by))
	if source.run ~= nil then
		put("run", s(source.run))
	end
	if source.turn ~= nil then
		put("turn", n(source.turn))
	end
	if source.commit ~= nil then
		put("commit", s(source.commit))
	end
	if source.files and #source.files > 0 then
		put("files", toml.strings(source.files))
	end
	if source.imported then
		local imported = source.imported
		table.insert(lines, "")
		table.insert(lines, "[source.imported]")
		put("from", s(imported.from))
		put("path", s(imported.path))
		if imported.session ~= nil then
			put("session", s(imported.session))
		end
	end
	for _, link in ipairs(note.links or {}) do
		table.insert(lines, "")
		table.insert(lines, "[[links]]")
		put("to", s(link.to))
		put("type", s(link.type))
		if link.why ~= nil then
			put("why", s(link.why))
		end
	end
	return "+++\n" .. table.concat(lines, "\n") .. "\n+++\n" .. note.body
end

-- Reading the front matter's values into a note, as serde did: a field
-- that is missing, or of the wrong kind, fails it; one it does not know
-- is left alone.

local function bad(why)
	error({ note = why }, 0)
end

local function value_of(fields, key)
	return fields[key]
end

local function string_of(fields, key, optional)
	local v = value_of(fields, key)
	if v == nil then
		if optional then
			return nil
		end
		bad("missing field `" .. key .. "`")
	end
	if v.string == nil then
		bad("`" .. key .. "` must be a string")
	end
	return v.string
end

local function integer_of(fields, key, optional, max)
	local v = value_of(fields, key)
	if v == nil then
		if optional then
			return nil
		end
		bad("missing field `" .. key .. "`")
	end
	if v.integer == nil then
		bad("`" .. key .. "` must be a whole number")
	end
	if v.integer < 0 or (max and v.integer > max) then
		bad("`" .. key .. "` is out of range")
	end
	return v.integer
end

local function strings_of(fields, key)
	local v = value_of(fields, key)
	if v == nil then
		return {}
	end
	if v.array == nil then
		bad("`" .. key .. "` must be a list")
	end
	local out = {}
	for _, item in ipairs(v.array) do
		if item.string == nil then
			bad("`" .. key .. "` must be a list of strings")
		end
		table.insert(out, item.string)
	end
	return out
end

local function one_of(fields, key, list, optional)
	local name = string_of(fields, key, optional)
	if name ~= nil and not member(list, name) then
		bad(string.format("unknown variant %q for `%s`", name, key))
	end
	return name
end

local function table_of(fields, key, optional)
	local v = value_of(fields, key)
	if v == nil then
		if optional then
			return nil
		end
		bad("missing field `" .. key .. "`")
	end
	if v.table == nil then
		bad("`" .. key .. "` must be a table")
	end
	return v.table.fields
end

-- The list of tables at `key`: `[[key]]` sections, or an inline array.
local function tables_of(fields, key)
	local v = value_of(fields, key)
	if v == nil then
		return {}
	end
	if v.tables then
		local out = {}
		for _, t in ipairs(v.tables) do
			table.insert(out, t.fields)
		end
		return out
	end
	if v.array then
		local out = {}
		for _, item in ipairs(v.array) do
			if item.table == nil then
				bad("`" .. key .. "` must be a list of tables")
			end
			table.insert(out, item.table.fields)
		end
		return out
	end
	bad("`" .. key .. "` must be a list of tables")
	return {}
end

local U32 = 4294967295

local function note_of(fields)
	local note = {
		id = string_of(fields, "id"),
		title = string_of(fields, "title"),
		description = string_of(fields, "description"),
		type = one_of(fields, "type", M.TYPES),
		tags = strings_of(fields, "tags"),
		created = integer_of(fields, "created"),
		updated = integer_of(fields, "updated"),
		valid_from = integer_of(fields, "valid_from"),
		valid_to = integer_of(fields, "valid_to", true),
		stale = string_of(fields, "stale", true),
		links = {},
	}
	local source = table_of(fields, "source")
	note.source = {
		by = one_of(source, "by", M.BY),
		run = string_of(source, "run", true),
		turn = integer_of(source, "turn", true, U32),
		commit = string_of(source, "commit", true),
		files = strings_of(source, "files"),
	}
	local imported = table_of(source, "imported", true)
	if imported then
		note.source.imported = {
			from = one_of(imported, "from", M.TOOLS),
			path = string_of(imported, "path"),
			session = string_of(imported, "session", true),
		}
	end
	for _, link in ipairs(tables_of(fields, "links")) do
		table.insert(note.links, {
			to = string_of(link, "to"),
			type = one_of(link, "type", M.LINKS),
			why = string_of(link, "why", true),
		})
	end
	return note
end

-- Reads a note file: the note, or nil and why it does not read.
function M.parse(file)
	if string.sub(file, 1, 4) ~= "+++\n" then
		return nil, "the file does not start with +++ front matter"
	end
	local rest = string.sub(file, 5)
	-- The front matter ends at the first line that is only the fence.
	local at = 1
	local front, body
	while true do
		local stop = string.find(rest, "\n", at, true)
		if stop == nil then
			return nil, "the file does not start with +++ front matter"
		end
		if string.sub(rest, at, stop - 1) == "+++" then
			front = string.sub(rest, 1, at - 1)
			body = string.sub(rest, stop + 1)
			break
		end
		at = stop + 1
	end
	local tree, why = toml.parse(front)
	if tree == nil then
		return nil, "bad front matter: " .. why
	end
	local ok, note = pcall(note_of, tree.fields)
	if not ok then
		if type(note) == "table" and note.note then
			return nil, "bad front matter: " .. note.note
		end
		error(note, 0)
	end
	note.body = body
	return note
end

-- The ids named by `[[id]]` in `s`, in order, each once. Anything
-- between the brackets that is not an id is left alone.
function M.wiki_links(s)
	local found = {}
	local at = 1
	while true do
		local open = string.find(s, "[[", at, true)
		if open == nil then
			break
		end
		local close = string.find(s, "]]", open + 2, true)
		if close == nil then
			break
		end
		local inner = string.sub(s, open + 2, close - 1)
		if M.is_id(inner) and not member(found, inner) then
			table.insert(found, inner)
		end
		at = close + 2
	end
	return found
end

-- Every link out of the note: those listed, then each bare `[[id]]` in
-- the body that is not listed already, as `relates`.
function M.all_links(note)
	local links = {}
	for _, link in ipairs(note.links or {}) do
		table.insert(links, link)
	end
	for _, to in ipairs(M.wiki_links(note.body)) do
		local listed = false
		for _, link in ipairs(links) do
			if link.to == to then
				listed = true
				break
			end
		end
		if not listed then
			table.insert(links, { to = to, type = "relates" })
		end
	end
	return links
end

-- Whether a newer note replaced this one.
function M.is_superseded(note)
	return note.valid_to ~= nil
end

-- A copy of `note`, its lists and tables its own.
function M.copy(note)
	local out = table.clone(note)
	out.tags = table.clone(note.tags or {})
	out.source = table.clone(note.source)
	out.source.files = table.clone(note.source.files or {})
	if note.source.imported then
		out.source.imported = table.clone(note.source.imported)
	end
	out.links = {}
	for _, link in ipairs(note.links or {}) do
		table.insert(out.links, table.clone(link))
	end
	return out
end

return M
