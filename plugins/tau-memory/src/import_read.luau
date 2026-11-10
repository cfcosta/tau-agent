-- Readers for what other coding agents kept on this computer (ADR 0044):
-- Claude Code's memory notes and chats, and Codex's memories, task
-- summaries and sessions.
--
-- Each reader takes `files` (`ctx.files`) and the person's home, never
-- the process's: the app gives the real one, tests a folder of their own.
-- Readers only read. A scan lists what there is, cheaply: paths, sizes,
-- the folder each was about. Reading an item gives a note as the tool
-- saved it (`{ note = … }`), or text for the person's model (`{ text,
-- offset }`). What a tool wrote is data: it is shown to the model fenced
-- as such, and every note made of it goes through the checks every note
-- does.
--
-- The formats change without notice, so the readers take only what they
-- need and skip what they cannot read: a file that does not read is
-- named, not fatal.
--
-- An item: `{ tool, material, path, key, place, about_you, fingerprint,
-- size, session, date, section }`, `tool` "claude_code" or "codex",
-- `material` "saved" or "chat", `place` `{ folder, remote }` or nil, its
-- `key` in the checkpoint the path, with `#` and the section for a part
-- of a file.

local read = {}

read.CLAUDE = "claude_code"
read.CODEX = "codex"
read.SAVED = "saved"
read.CHAT = "chat"

-- The index Claude Code keeps beside its notes: not a note.
read.INDEX = "MEMORY.md"

-- Codex's record of the Claude Code chats it imported.
read.IMPORTS = "external_agent_session_imports.json"

-- How much of a tool call's arguments, and of its result, a chat shows
-- the model.
local CALL_CHARS = 300
local RESULT_CHARS = 400

-- How many of a chat's first lines are looked at for where and when it
-- ran.
local HEAD_LINES = 64

-- The most one read takes of a file: what `ctx.files.read` gives.
read.MAX_BYTES = 8 * 1024 * 1024

-- What a head's first read takes; it reads more while it lacks lines.
local HEAD_BYTES = 64 * 1024

-- Text ---------------------------------------------------------------

-- White space as Unicode has it, as Rust's `trim` and `split_whitespace`
-- read it.
local WIDE_SPACES = {}
for _, c in ipairs({ 0x85, 0xA0, 0x1680, 0x2028, 0x2029, 0x202F, 0x205F, 0x3000 }) do
	table.insert(WIDE_SPACES, utf8.char(c))
end
for c = 0x2000, 0x200A do
	table.insert(WIDE_SPACES, utf8.char(c))
end
local SPACE = {}
for _, c in ipairs({ 0x20, 0x85, 0xA0, 0x1680, 0x2028, 0x2029, 0x202F, 0x205F, 0x3000 }) do
	SPACE[c] = true
end
for c = 0x09, 0x0D do
	SPACE[c] = true
end
for c = 0x2000, 0x200A do
	SPACE[c] = true
end

-- `text` with every run of white space one space, none at its ends.
function read.one_line(text)
	for _, space in ipairs(WIDE_SPACES) do
		if string.find(text, space, 1, true) then
			text = string.gsub(text, space, " ")
		end
	end
	text = string.gsub(text, "%s+", " ")
	if string.sub(text, 1, 1) == " " then
		text = string.sub(text, 2)
	end
	if string.sub(text, -1) == " " then
		text = string.sub(text, 1, -2)
	end
	return text
end

-- `text` without white space at either end.
function read.trim(text)
	local first = 1
	while first <= #text do
		local c = utf8.codepoint(text, first)
		if not SPACE[c] then
			break
		end
		first = utf8.offset(text, 2, first)
	end
	local last = #text
	while last >= first do
		local start = last
		while start > first and bit32.band(string.byte(text, start), 0xC0) == 0x80 do
			start -= 1
		end
		if not SPACE[utf8.codepoint(text, start)] then
			break
		end
		last = start - 1
	end
	return string.sub(text, first, last)
end

-- `text` without white space at its start.
local function trim_start(text)
	local first = 1
	while first <= #text do
		local c = utf8.codepoint(text, first)
		if not SPACE[c] then
			break
		end
		first = utf8.offset(text, 2, first)
	end
	return string.sub(text, first)
end

-- Whether `text` starts with `prefix`.
local function starts(text, prefix)
	return string.sub(text, 1, #prefix) == prefix
end

-- Whether `text` ends with `suffix`.
local function ends(text, suffix)
	return suffix == "" or string.sub(text, -#suffix) == suffix
end

-- The lines of `text` as Rust's `str::lines` gives them: split at `\n`,
-- a `\r` before it dropped, no empty line after a last `\n`.
function read.lines(text)
	local out = {}
	local at = 1
	while at <= #text do
		local stop = string.find(text, "\n", at, true)
		local line
		if stop == nil then
			line = string.sub(text, at)
			at = #text + 1
		else
			line = string.sub(text, at, stop - 1)
			at = stop + 1
		end
		if string.sub(line, -1) == "\r" then
			line = string.sub(line, 1, -2)
		end
		table.insert(out, line)
	end
	return out
end

-- Whether `a` sorts before `b` byte by byte, as Rust compares strings.
local function bytes_less(a, b)
	local n = math.min(#a, #b)
	for i = 1, n do
		local x, y = string.byte(a, i), string.byte(b, i)
		if x ~= y then
			return x < y
		end
	end
	return #a < #b
end
read.bytes_less = bytes_less

-- The components of a path, as Rust's `Path::components` gives them for
-- the plain paths a tool writes: `/` first for an absolute one.
local function components(path)
	local out = {}
	if starts(path, "/") then
		table.insert(out, "/")
	end
	for part in string.gmatch(path, "[^/]+") do
		if part ~= "." or #out == 0 then
			table.insert(out, part)
		end
	end
	return out
end

-- Whether path `a` sorts before path `b`, component by component.
function read.path_less(a, b)
	local x, y = components(a), components(b)
	for i = 1, math.min(#x, #y) do
		if x[i] ~= y[i] then
			return bytes_less(x[i], y[i])
		end
	end
	return #x < #y
end

-- Whether the path `path` is `base` or inside it, component by component.
function read.path_under(path, base)
	local x, y = components(path), components(base)
	if #y > #x then
		return false
	end
	for i = 1, #y do
		if x[i] ~= y[i] then
			return false
		end
	end
	return true
end

-- `dir` and `name` joined.
local function join(dir, name)
	if ends(dir, "/") then
		return dir .. name
	end
	return dir .. "/" .. name
end
read.join = join

-- The last part of `path`.
local function file_name(path)
	return string.match(path, "([^/]+)/*$") or path
end

-- A file name's extension, as Rust's `Path::extension` reads it: after
-- the last `.`, unless that dot starts the name.
local function extension(name)
	local dot = string.match(name, "^.*()%.")
	if dot == nil or dot == 1 then
		return nil
	end
	return string.sub(name, dot + 1)
end

-- A file name without its extension, as Rust's `Path::file_stem` gives it.
local function stem(name)
	local dot = string.match(name, "^.*()%.")
	if dot == nil or dot == 1 then
		return name
	end
	return string.sub(name, 1, dot - 1)
end

-- `path` as the person writes it: under `~` when it is in `home`.
function read.show(home, path)
	if home ~= nil and read.path_under(path, home) then
		local rest = string.sub(path, #home + 1)
		rest = string.gsub(rest, "^/+", "")
		return "~/" .. rest
	end
	return path
end

-- `text` cut to `max` characters, marked when it was.
function read.cut(text, max)
	if utf8.len(text) <= max then
		return text
	end
	return string.sub(text, 1, utf8.offset(text, max + 1) - 1) .. "… (cut)"
end

-- The day of an ISO 8601 time: `2026-10-04` of `2026-10-04T12:00:00Z`.
function read.day(time)
	if type(time) ~= "string" then
		return nil
	end
	local day = string.sub(time, 1, 10)
	if string.match(day, "^%d%d%d%d%-%d%d%-%d%d$") == nil then
		return nil
	end
	return day
end

-- FNV-1a, 64 bits, over `text`'s bytes, as 16 hex digits: stable across
-- runs and builds. The hash is two 32-bit halves, as Luau's numbers hold
-- 53 bits.
function read.fnv(text)
	local hi, lo = 0xcbf29ce4, 0x84222325
	for i = 1, #text do
		lo = bit32.bxor(lo, string.byte(text, i))
		-- × 0x100000001b3: × 0x1b3, and the low half shifted 40 bits up.
		local low = lo * 0x1b3
		local carry = low // 0x100000000
		local high = hi * 0x1b3 + carry + (lo * 0x100) % 0x100000000
		lo = low % 0x100000000
		hi = high % 0x100000000
	end
	return string.format("%08x%08x", hi, lo)
end

-- A hash of a section's text, for its fingerprint.
function read.of_text(text)
	return "h" .. read.fnv(text)
end

-- A file's size and modification time, as a listing gives them.
local function of_file(entry)
	return string.format("%d:%d", entry.size, entry.modified or 0)
end

-- Cuts `text` into chunks of at most `max` bytes (or one character, when
-- `max` is less), between entries (a blank line before a `[`) where it
-- can, else between lines, else anywhere on a character boundary.
-- Joined, they are `text`.
function read.chunks(text, max)
	max = math.max(max, 1)
	local out = {}
	local rest = text
	while #rest > max do
		local cut = max
		while cut > 0 and cut < #rest and bit32.band(string.byte(rest, cut + 1), 0xC0) == 0x80 do
			cut -= 1
		end
		local window = string.sub(rest, 1, cut)
		local entry = nil
		local from = 1
		while true do
			local found = string.find(window, "\n\n[", from, true)
			if found == nil then
				break
			end
			entry = found
			from = found + 1
		end
		local line = string.match(window, "^.*()\n")
		if entry ~= nil and entry > 1 then
			cut = entry - 1 + 2
		elseif line ~= nil and line > 1 then
			cut = line
		end
		if cut == 0 then
			local second = utf8.offset(rest, 2)
			cut = second and second - 1 or 1
		end
		table.insert(out, string.sub(rest, 1, cut))
		rest = string.sub(rest, cut + 1)
	end
	if rest ~= "" then
		table.insert(out, rest)
	end
	return out
end

-- JSON in the order it was written ------------------------------------

-- serde's compact text of a JSON value, its keys in the order the line
-- wrote them: `json.decode` keeps no order, and the model sees a call's
-- arguments as Rust wrote them (`serde_json` with `preserve_order`).
local ordered = {}

local function skip_space(text, at)
	return string.find(text, "[^ \t\n\r]", at) or #text + 1
end

-- serde's escapes for a string's text.
local ESCAPES = { ['"'] = '\\"', ["\\"] = "\\\\", ["\b"] = "\\b", ["\f"] = "\\f", ["\n"] = "\\n", ["\r"] = "\\r", ["\t"] = "\\t" }
local function escape(text)
	return '"'
		.. string.gsub(text, '[\0-\31"\\]', function(c)
			return ESCAPES[c] or string.format("\\u%04x", string.byte(c))
		end)
		.. '"'
end

-- The string at `at` (its opening quote): its text and where it ends.
local function parse_string(text, at)
	local out = {}
	local i = at + 1
	while true do
		local stop = string.find(text, '["\\]', i)
		if stop == nil then
			error("unterminated string")
		end
		table.insert(out, string.sub(text, i, stop - 1))
		if string.sub(text, stop, stop) == '"' then
			return table.concat(out), stop + 1
		end
		local kind = string.sub(text, stop + 1, stop + 1)
		local simple = { ['"'] = '"', ["\\"] = "\\", ["/"] = "/", b = "\b", f = "\f", n = "\n", r = "\r", t = "\t" }
		if simple[kind] then
			table.insert(out, simple[kind])
			i = stop + 2
		elseif kind == "u" then
			local code = tonumber(string.sub(text, stop + 2, stop + 5), 16)
			i = stop + 6
			if code and code >= 0xD800 and code <= 0xDBFF and string.sub(text, i, i + 1) == "\\u" then
				local low = tonumber(string.sub(text, i + 2, i + 5), 16)
				if low and low >= 0xDC00 and low <= 0xDFFF then
					code = 0x10000 + (code - 0xD800) * 0x400 + (low - 0xDC00)
					i += 6
				end
			end
			table.insert(out, utf8.char(code or 0xFFFD))
		else
			error("bad escape")
		end
	end
end

-- serde's text of a number as JSON wrote it: an integer stays, a float is
-- Rust's shortest form that reads back.
local function number_text(raw)
	if string.match(raw, "^%-?%d+$") then
		return raw
	end
	local value = tonumber(raw)
	if value == nil then
		return raw
	end
	if value == math.floor(value) and math.abs(value) < 1e16 then
		return string.format("%d", value) .. ".0"
	end
	for digits = 1, 17 do
		local shown = string.format("%." .. digits .. "g", value)
		if tonumber(shown) == value then
			if string.find(shown, "e") then
				local mantissa, exponent = string.match(shown, "^(.-)e([-+]%d+)$")
				return mantissa .. "e" .. tostring(tonumber(exponent))
			end
			return shown
		end
	end
	return raw
end

-- The value at `at`: `{ kind, … }` and where it ends.
local function parse_value(text, at)
	at = skip_space(text, at)
	local c = string.sub(text, at, at)
	if c == "{" then
		local keys, values = {}, {}
		at = skip_space(text, at + 1)
		if string.sub(text, at, at) == "}" then
			return { kind = "object", keys = keys, values = values }, at + 1
		end
		while true do
			at = skip_space(text, at)
			if string.sub(text, at, at) ~= '"' then
				error("expected a key")
			end
			local key
			key, at = parse_string(text, at)
			at = skip_space(text, at)
			if string.sub(text, at, at) ~= ":" then
				error("expected :")
			end
			local value
			value, at = parse_value(text, at + 1)
			-- A key written twice keeps its first place and its last value.
			if values[key] == nil then
				table.insert(keys, key)
			end
			values[key] = value
			at = skip_space(text, at)
			local next = string.sub(text, at, at)
			if next == "}" then
				return { kind = "object", keys = keys, values = values }, at + 1
			elseif next ~= "," then
				error("expected , or }")
			end
			at += 1
		end
	elseif c == "[" then
		local items = {}
		at = skip_space(text, at + 1)
		if string.sub(text, at, at) == "]" then
			return { kind = "array", items = items }, at + 1
		end
		while true do
			local value
			value, at = parse_value(text, at)
			table.insert(items, value)
			at = skip_space(text, at)
			local next = string.sub(text, at, at)
			if next == "]" then
				return { kind = "array", items = items }, at + 1
			elseif next ~= "," then
				error("expected , or ]")
			end
			at += 1
		end
	elseif c == '"' then
		local value
		value, at = parse_string(text, at)
		return { kind = "string", value = value }, at
	elseif string.sub(text, at, at + 3) == "true" then
		return { kind = "raw", text = "true" }, at + 4
	elseif string.sub(text, at, at + 4) == "false" then
		return { kind = "raw", text = "false" }, at + 5
	elseif string.sub(text, at, at + 3) == "null" then
		return { kind = "raw", text = "null" }, at + 4
	end
	local raw = string.match(text, "^%-?%d+%.?%d*[eE]?[-+]?%d*", at)
	if raw == nil or raw == "" then
		error("expected a value")
	end
	return { kind = "raw", text = number_text(raw) }, at + #raw
end

-- The line parsed in order; nil when it is no JSON.
function ordered.parse(text)
	local ok, value, at = pcall(parse_value, text, 1)
	if not ok or skip_space(text, at) <= #text then
		return nil
	end
	return value
end

-- serde's compact text of `value`.
function ordered.text(value)
	if value.kind == "object" then
		local parts = {}
		for _, key in ipairs(value.keys) do
			table.insert(parts, escape(key) .. ":" .. ordered.text(value.values[key]))
		end
		return "{" .. table.concat(parts, ",") .. "}"
	elseif value.kind == "array" then
		local parts = {}
		for _, item in ipairs(value.items) do
			table.insert(parts, ordered.text(item))
		end
		return "[" .. table.concat(parts, ",") .. "]"
	elseif value.kind == "string" then
		return escape(value.value)
	end
	return value.text
end

-- The field `key` of an ordered object, or nil.
function ordered.get(value, key)
	if value == nil or value.kind ~= "object" then
		return nil
	end
	return value.values[key]
end

read.ordered = ordered

-- JSON ----------------------------------------------------------------

-- A decoded line, or nil when it is not JSON.
local function decode(line)
	local ok, value = pcall(json.decode, line)
	if not ok then
		return nil
	end
	return value
end

-- `value` when it is a string.
local function str(value)
	if type(value) == "string" then
		return value
	end
	return nil
end

-- A table's field, nil for anything not a table.
local function field(value, key)
	if type(value) ~= "table" then
		return nil
	end
	local found = value[key]
	if found == json.null then
		return nil
	end
	return found
end

-- Whether a decoded value is a JSON array.
local function is_array(value)
	if type(value) ~= "table" then
		return false
	end
	if getmetatable(value) == getmetatable(array({})) then
		return true
	end
	-- An object has string keys; an array has none of them.
	if next(value) == nil then
		return false
	end
	for key in pairs(value) do
		if type(key) ~= "number" then
			return false
		end
	end
	return true
end

-- Files ---------------------------------------------------------------

-- What `dir` holds, `{ name, dir, size, modified }` each, by name; none
-- when it does not list.
local function list(files, dir)
	local ok, found = pcall(files.list, dir)
	if not ok or type(found) ~= "table" then
		return {}
	end
	return found
end

-- A file's text, or nil and why.
local function text_of(files, path)
	local ok, text = pcall(files.read, path)
	if not ok then
		return nil, tostring(text)
	end
	return text
end

-- The files in `dir` ending in `.ext`, `{ path, entry }` each, sorted.
local function files_in(files, dir, ext)
	local found = {}
	for _, entry in ipairs(list(files, dir)) do
		if not entry.dir and extension(entry.name) == ext then
			table.insert(found, { path = join(dir, entry.name), entry = entry })
		end
	end
	table.sort(found, function(a, b)
		return read.path_less(a.path, b.path)
	end)
	return found
end

-- Each folder's place, its remote read once.
function read.remotes(files)
	local known = {}
	return function(folder)
		if known[folder] == nil then
			known[folder] = { remote = read.remote(files, folder) or false }
		end
		return { folder = folder, remote = known[folder].remote or nil }
	end
end

-- The remote of the repository holding `folder`, as its config writes it:
-- of the nearest folder above it that is there when it is gone: the path
-- is walked up.
function read.remote(files, folder)
	local path = folder
	while true do
		local ok, found = pcall(files.remote, path)
		if ok then
			return found and found.url or nil
		end
		if path == "/" then
			return nil
		end
		local parent = string.match(path, "^(.*)/[^/]*$")
		if parent == nil then
			return nil
		end
		path = parent == "" and "/" or parent
	end
end

-- The whole lines of the file at `path` from `offset`, and the offset
-- after them: what a chat read again later starts at. A file that shrank
-- below `offset` (its `size` now) was written again: it is read whole. At
-- most one read's bytes are taken; the rest waits for the next.
function read.lines_from(files, path, offset, size)
	if offset > size then
		offset = 0
	end
	local text = files.read(path, { offset = offset, limit = read.MAX_BYTES, lossy = true })
	local whole = string.match(text, "^.*()\n") or 0
	if whole > 0 and string.find(text, "\u{FFFD}", 1, true) then
		-- Bytes that were not UTF-8 read as more bytes than they were:
		-- how far the lines go is counted on the bytes themselves.
		local raw = read.base64(files.read(path, { offset = offset, limit = read.MAX_BYTES, bytes = true }))
		local last = string.match(raw, "^.*()\n") or 0
		text = files.read(path, { offset = offset, limit = last, lossy = true })
		return text, offset + last
	end
	return string.sub(text, 1, whole), offset + whole
end

-- The bytes base64 `text` holds.
local B64 = {}
do
	local letters = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
	for i = 1, #letters do
		B64[string.byte(letters, i)] = i - 1
	end
end
function read.base64(text)
	local out = {}
	local bits, count = 0, 0
	for i = 1, #text do
		local value = B64[string.byte(text, i)]
		if value ~= nil then
			bits = bits * 64 + value
			count += 6
			if count >= 8 then
				count -= 8
				local byte = bits // (2 ^ count)
				table.insert(out, string.char(byte))
				bits -= byte * (2 ^ count)
			end
		end
	end
	return table.concat(out)
end

-- The first `count` whole lines of the file at `path`, read a little at a
-- time: a chat's head without the whole of it.
local function first_lines(files, path, size, count)
	local limit = HEAD_BYTES
	while true do
		local cap = math.min(limit, read.MAX_BYTES)
		local ok, text = pcall(files.read, path, { limit = cap, lossy = true })
		if not ok then
			return {}
		end
		local all = cap >= size or cap >= read.MAX_BYTES
		local lines = read.lines(text)
		if not all and string.sub(text, -1) ~= "\n" then
			-- The last line may go on past what was read.
			table.remove(lines)
		end
		if #lines >= count or all then
			-- Rust's `BufRead::lines` stops at a line that is not UTF-8, which
			-- the lossy text cannot tell from one holding U+FFFD: then the
			-- lines are read again as bytes.
			local raw = nil
			if string.find(text, "\u{FFFD}", 1, true) then
				local read_ok, bytes = pcall(files.read, path, { limit = cap, bytes = true })
				raw = read_ok and read.lines(read.base64(bytes)) or {}
			end
			local out = {}
			for i = 1, math.min(count, #lines) do
				if raw ~= nil and (raw[i] == nil or utf8.len(raw[i]) == nil) then
					break
				end
				out[i] = lines[i]
			end
			return out
		end
		limit *= 4
	end
end

-- Claude Code -----------------------------------------------------------

-- The name Claude Code gives the folder of `path`'s chats: every character
-- but an ASCII letter or digit written `-`.
function read.encode(path)
	local out = {}
	for _, c in utf8.codes(path) do
		local ascii = c < 128 and string.match(string.char(c), "%w") ~= nil
		table.insert(out, ascii and string.char(c) or "-")
	end
	return table.concat(out)
end

-- The existing folder whose `encode` is `name`, found by walking from `/`
-- along folders whose names fit; nil when none does.
function read.decode(files, name)
	local function walk(dir, rest)
		if rest == "" then
			return dir
		end
		-- Every component starts with what `/` became.
		if string.sub(rest, 1, 1) ~= "-" then
			return nil
		end
		rest = string.sub(rest, 2)
		local fits = {}
		for _, entry in ipairs(list(files, dir)) do
			local encoded = read.encode(entry.name)
			if starts(rest, encoded) then
				local after = string.sub(rest, #encoded + 1)
				if after == "" or string.sub(after, 1, 1) == "-" then
					table.insert(fits, { name = entry.name, len = #encoded, dir = entry.dir })
				end
			end
		end
		-- The longest first: `tau-agent` before `tau`.
		table.sort(fits, function(a, b)
			if a.len ~= b.len then
				return a.len > b.len
			end
			return bytes_less(a.name, b.name)
		end)
		for _, fit in ipairs(fits) do
			if fit.dir then
				local found = walk(join(dir, fit.name), string.sub(rest, fit.len + 1))
				if found ~= nil then
					return found
				end
			end
		end
		return nil
	end
	return walk("/", name)
end

-- `value` unquoted, as Claude Code's notes quote their front matter.
local function unquote(value)
	local function quoted(q)
		return #value >= 2 and starts(value, q) and ends(value, q)
	end
	if quoted('"') then
		return (string.gsub(string.sub(value, 2, -2), '\\"', '"'))
	elseif quoted("'") then
		return (string.gsub(string.sub(value, 2, -2), "''", "'"))
	end
	return value
end

-- The keys of simple YAML front matter, a nested key as `parent.child`,
-- with their values unquoted; a block scalar (`|`, `>`) is its lines
-- joined.
local function front_matter(front)
	local fields = {}
	local parent = nil
	local block = nil
	for _, line in ipairs(read.lines(front)) do
		local indent = #line - #trim_start(line)
		local continued = false
		if block ~= nil then
			if indent > block.at and read.trim(line) ~= "" then
				for _, each in ipairs(fields) do
					if each[1] == block.key then
						if each[2] ~= "" then
							each[2] ..= " "
						end
						each[2] ..= read.trim(line)
						break
					end
				end
				continued = true
			else
				block = nil
			end
		end
		if not continued then
			local trimmed = read.trim(line)
			local colon = string.find(trimmed, ":", 1, true)
			if colon ~= nil then
				local key = read.trim(string.sub(trimmed, 1, colon - 1))
				local value = read.trim(string.sub(trimmed, colon + 1))
				if key ~= "" and not starts(key, "#") and not starts(key, "-") then
					local full = key
					if parent ~= nil and indent >= 1 then
						full = parent .. "." .. key
					end
					if indent == 0 then
						parent = value == "" and key or nil
					end
					if starts(value, "|") or starts(value, ">") then
						block = { key = full, at = indent }
						table.insert(fields, { full, "" })
					else
						table.insert(fields, { full, unquote(value) })
					end
				end
			end
		end
	end
	return fields
end

-- A memory note file: its front matter between `---` lines, then its
-- body. `path` names it when it has no `name`. Gives `{ kind, title,
-- description, body, about_you }`, or nil and why.
function read.parse_note(text, path)
	text = string.gsub(text, "\r\n", "\n")
	if not starts(text, "---\n") then
		return nil, "no front matter"
	end
	local rest = string.sub(text, 5)
	local front, body
	local at = string.find(rest, "\n---", 1, true)
	if at ~= nil then
		local after = string.sub(rest, at + 4)
		local newline = string.find(after, "\n", 1, true)
		body = newline and string.sub(after, newline + 1) or ""
		front = string.sub(rest, 1, at - 1)
	elseif starts(rest, "---") then
		front, body = "", (string.gsub(rest, "^%-+", ""))
	else
		return nil, "front matter never ends"
	end
	local fields = front_matter(front)
	local function get(key)
		for _, each in ipairs(fields) do
			if each[1] == key then
				return each[2] ~= "" and each[2] or nil
			end
		end
		return nil
	end
	local kind = get("metadata.type") or get("type") or ""
	local about_you = kind == "user" or kind == "feedback"
	body = read.trim(body)
	local name = stem(file_name(path))
	name = string.gsub(name, "[-_]", " ")
	local title = read.one_line(get("name") or name)
	if title == "" then
		return nil, "no name"
	end
	local description = get("description") and read.one_line(get("description")) or ""
	if description == "" then
		description = nil
		for _, line in ipairs(read.lines(body)) do
			if read.trim(line) ~= "" then
				description = read.one_line(line)
				break
			end
		end
		description = description or title
	end
	return {
		kind = about_you and "preference" or "fact",
		title = title,
		description = description,
		body = body,
		about_you = about_you,
	}
end

-- What the start of a chat says: where it ran, and when.
local function head(files, path, size)
	local found = {}
	for _, line in ipairs(first_lines(files, path, size, HEAD_LINES)) do
		local event = decode(line)
		if type(event) == "table" then
			if found.cwd == nil then
				found.cwd = str(field(event, "cwd"))
			end
			if found.date == nil then
				found.date = read.day(field(event, "timestamp"))
			end
			if found.cwd ~= nil and found.date ~= nil then
				break
			end
		end
	end
	return found
end

-- Every note and chat under `root` (`~/.claude`).
local function scan_claude(files, root, place_of, scan)
	local projects = {}
	for _, entry in ipairs(list(files, join(root, "projects"))) do
		if entry.dir then
			table.insert(projects, { path = join(join(root, "projects"), entry.name), name = entry.name })
		end
	end
	table.sort(projects, function(a, b)
		return read.path_less(a.path, b.path)
	end)
	for _, project in ipairs(projects) do
		local chats = {}
		for _, chat in ipairs(files_in(files, project.path, "jsonl")) do
			chat.head = head(files, chat.path, chat.entry.size)
			table.insert(chats, chat)
		end
		local folder = nil
		for _, chat in ipairs(chats) do
			if chat.head.cwd ~= nil then
				folder = chat.head.cwd
				break
			end
		end
		folder = folder or read.decode(files, project.name)
		local place = folder and place_of(folder) or nil
		for _, note in ipairs(files_in(files, join(project.path, "memory"), "md")) do
			if file_name(note.path) ~= read.INDEX then
				local text = text_of(files, note.path)
				local parsed = text and read.parse_note(text, note.path)
				if parsed == nil then
					table.insert(scan.failed, note.path)
				else
					table.insert(scan.items, {
						tool = read.CLAUDE,
						material = read.SAVED,
						key = note.path,
						path = note.path,
						place = place,
						about_you = parsed.about_you,
						fingerprint = of_file(note.entry),
						size = note.entry.size,
					})
				end
			end
		end
		for _, chat in ipairs(chats) do
			table.insert(scan.items, {
				tool = read.CLAUDE,
				material = read.CHAT,
				key = chat.path,
				path = chat.path,
				session = stem(file_name(chat.path)),
				place = place,
				about_you = false,
				fingerprint = of_file(chat.entry),
				size = chat.entry.size,
				date = chat.head.date,
			})
		end
	end
end

-- `text` without `<system-reminder>` blocks.
local function without_reminders(text)
	local OPEN, CLOSE = "<system-reminder>", "</system-reminder>"
	local out = {}
	local rest = text
	while true do
		local at = string.find(rest, OPEN, 1, true)
		if at == nil then
			break
		end
		table.insert(out, string.sub(rest, 1, at - 1))
		local close = string.find(rest, CLOSE, at, true)
		if close == nil then
			rest = ""
		else
			rest = string.sub(rest, close + #CLOSE)
		end
	end
	table.insert(out, rest)
	return table.concat(out)
end

-- What `who` said, without the reminders tools add to it.
local function said_by(out, who, said)
	said = read.trim(without_reminders(said))
	if said ~= "" then
		table.insert(out, "[" .. who .. "]\n" .. said .. "\n\n")
	end
end

-- A Claude Code tool call's arguments, as serde writes them from the line.
local function call_input(line, n)
	local event = ordered.parse(line)
	local blocks = ordered.get(ordered.get(event, "message"), "content")
	local block = blocks and blocks.kind == "array" and blocks.items[n] or nil
	local input = ordered.get(block, "input")
	return input and ordered.text(input) or "null"
end

-- A chat's events from `offset` on, as text for the model: what the
-- person and the agent said, the calls it made and the start of what they
-- returned. Only whole lines are read; the offset given back is after the
-- last.
local function read_claude_chat(files, item, offset)
	local lines, after = read.lines_from(files, item.path, offset, item.size)
	local out = {}
	local events = 0
	for _, line in ipairs(read.lines(lines)) do
		local event = decode(line)
		if event ~= nil then
			events += 1
			local skip = field(event, "isSidechain") == true or field(event, "isMeta") == true
			local content = field(field(event, "message"), "content")
			local kind = field(event, "type")
			if not skip and kind == "user" then
				if type(content) == "string" then
					said_by(out, "user", content)
				elseif is_array(content) then
					for _, block in ipairs(content) do
						local what = field(block, "type")
						if what == "text" then
							said_by(out, "user", str(field(block, "text")) or "")
						elseif what == "tool_result" then
							local result = field(block, "content")
							local text = ""
							if type(result) == "string" then
								text = result
							elseif is_array(result) then
								local parts = {}
								for _, part in ipairs(result) do
									local said = str(field(part, "text"))
									if said ~= nil then
										table.insert(parts, said)
									end
								end
								text = table.concat(parts, "\n")
							end
							table.insert(out, "[result]\n" .. read.cut(read.trim(text), RESULT_CHARS) .. "\n\n")
						end
					end
				end
			elseif not skip and kind == "assistant" and is_array(content) then
				for n, block in ipairs(content) do
					local what = field(block, "type")
					if what == "text" then
						said_by(out, "assistant", str(field(block, "text")) or "")
					elseif what == "tool_use" then
						local name = str(field(block, "name")) or "tool"
						table.insert(out, "[call " .. name .. "] " .. read.cut(call_input(line, n), CALL_CHARS) .. "\n\n")
					end
				end
			end
		end
	end
	if events == 0 and read.trim(lines) ~= "" then
		return nil, "not a Claude Code chat"
	end
	return { text = table.concat(out), offset = after }
end

-- Codex -----------------------------------------------------------------

-- The sections of `text` under lines starting with `marker`: what follows
-- the marker on its line, and the section, that line included.
local function sections(text, marker)
	local out = {}
	for _, line in ipairs(read.lines(text)) do
		if starts(line, marker) then
			table.insert(out, { heading = read.trim(string.sub(line, #marker + 1)), parts = {} })
		end
		local last = out[#out]
		if last ~= nil then
			table.insert(last.parts, line .. "\n")
		end
	end
	for _, section in ipairs(out) do
		section.text = table.concat(section.parts)
		section.parts = nil
	end
	return out
end

-- The `key: value` lines at the start of `text`, up to the first line that
-- is not one.
local function header(text)
	local found = {}
	for _, line in ipairs(read.lines(text)) do
		local colon = string.find(line, ":", 1, true)
		if colon == nil then
			break
		end
		local key = read.trim(string.sub(line, 1, colon - 1))
		if key == "" or string.match(key, "^[a-z_]+$") == nil then
			break
		end
		found[key] = read.trim(string.sub(line, colon + 1))
	end
	return found
end

-- The ids of the sessions Codex made of Claude Code's chats.
local function imported(files, root)
	local found = {}
	local text = text_of(files, join(root, read.IMPORTS))
	local imports = text and decode(text)
	local records = field(imports, "records")
	if is_array(records) then
		for _, record in ipairs(records) do
			local id = str(field(record, "imported_thread_id"))
			if id ~= nil then
				found[id] = true
			end
		end
	end
	return found
end

-- Every `.jsonl` under `dir`, its listing entry with it.
local function jsonl_under(files, dir, out)
	for _, entry in ipairs(list(files, dir)) do
		local path = join(dir, entry.name)
		if entry.dir then
			jsonl_under(files, path, out)
		elseif extension(entry.name) == "jsonl" then
			table.insert(out, { path = path, entry = entry })
		end
	end
end

-- What a session's first line says, or nil.
local function session_meta(files, path, size)
	local first = first_lines(files, path, size, 1)[1]
	local event = first and decode(first)
	if type(event) ~= "table" or field(event, "type") ~= "session_meta" then
		return nil
	end
	local meta = field(event, "payload")
	local source = field(meta, "source")
	return {
		id = str(field(meta, "id")),
		cwd = str(field(meta, "cwd")),
		date = read.day(str(field(meta, "timestamp")) or str(field(event, "timestamp"))),
		subagent = field(meta, "thread_source") == "subagent" or (type(source) == "table" and source.subagent ~= nil),
	}
end

-- The file's listing entry, or nil when it is not there.
local function entry_of(files, path)
	local dir, name = string.match(path, "^(.*)/([^/]+)$")
	if dir == nil then
		return nil
	end
	for _, entry in ipairs(list(files, dir == "" and "/" or dir)) do
		if entry.name == name and not entry.dir then
			return entry
		end
	end
	return nil
end

-- The items of the memory folder.
local function memories(files, dir, place_of, scan)
	local index = join(dir, "MEMORY.md")
	local listed = {}
	local text = text_of(files, index)
	if text ~= nil then
		for n, section in ipairs(sections(text, "# Task Group:")) do
			for word in string.gmatch(read.one_line(section.text), "%S+") do
				local at = string.find(word, "rollout_summaries/", 1, true)
				if at ~= nil then
					local name = string.sub(word, at + #"rollout_summaries/")
					name = string.gsub(name, "[,);]+$", "")
					listed[name] = true
				end
			end
			local heading = section.heading
			local dash = string.find(heading, " — ", 1, true)
			local cwd = read.trim(dash and string.sub(heading, 1, dash - 1) or heading)
			table.insert(scan.items, {
				tool = read.CODEX,
				material = read.SAVED,
				key = index .. "#" .. (n - 1),
				path = index,
				place = starts(cwd, "/") and place_of(cwd) or nil,
				about_you = false,
				fingerprint = read.of_text(section.text),
				size = #section.text,
				section = section.text,
			})
		end
	end
	local summary = join(dir, "memory_summary.md")
	local entry = entry_of(files, summary)
	if entry ~= nil then
		table.insert(scan.items, {
			tool = read.CODEX,
			material = read.SAVED,
			key = summary,
			path = summary,
			about_you = true,
			fingerprint = of_file(entry),
			size = entry.size,
		})
	end
	local threads = {}
	for _, rollout in ipairs(files_in(files, join(dir, "rollout_summaries"), "md")) do
		local body = text_of(files, rollout.path)
		if body == nil then
			table.insert(scan.failed, rollout.path)
		else
			local found = header(body)
			if found.thread_id ~= nil then
				threads[found.thread_id] = true
			end
			if not listed[file_name(rollout.path)] then
				table.insert(scan.items, {
					tool = read.CODEX,
					material = read.SAVED,
					key = rollout.path,
					path = rollout.path,
					place = found.cwd and place_of(found.cwd) or nil,
					about_you = false,
					fingerprint = of_file(rollout.entry),
					size = rollout.entry.size,
					session = found.thread_id,
					date = found.updated_at and read.day(found.updated_at) or nil,
				})
			end
		end
	end
	local raw = join(dir, "raw_memories.md")
	text = text_of(files, raw)
	if text ~= nil then
		for _, section in ipairs(sections(text, "## Thread")) do
			local thread = string.gsub(read.trim(section.heading), "^`+", "")
			thread = string.gsub(thread, "`+$", "")
			if not threads[thread] then
				local newline = string.find(section.text, "\n", 1, true)
				local found = header(newline and string.sub(section.text, newline + 1) or "")
				table.insert(scan.items, {
					tool = read.CODEX,
					material = read.SAVED,
					key = raw .. "#" .. thread,
					path = raw,
					place = found.cwd and place_of(found.cwd) or nil,
					about_you = false,
					fingerprint = read.of_text(section.text),
					size = #section.text,
					session = thread,
					date = found.updated_at and read.day(found.updated_at) or nil,
					section = section.text,
				})
			end
		end
	end
end

-- Everything under `root` (`~/.codex`).
local function scan_codex(files, root, place_of, scan)
	memories(files, join(root, "memories"), place_of, scan)
	local skip = imported(files, root)
	local sessions = {}
	jsonl_under(files, join(root, "sessions"), sessions)
	table.sort(sessions, function(a, b)
		return read.path_less(a.path, b.path)
	end)
	for _, session in ipairs(sessions) do
		local meta = session_meta(files, session.path, session.entry.size)
		if meta == nil then
			table.insert(scan.failed, session.path)
		elseif not meta.subagent and not (meta.id and skip[meta.id]) then
			table.insert(scan.items, {
				tool = read.CODEX,
				material = read.CHAT,
				key = session.path,
				path = session.path,
				place = meta.cwd and place_of(meta.cwd) or nil,
				about_you = false,
				fingerprint = of_file(session.entry),
				size = session.entry.size,
				session = meta.id,
				date = meta.date,
			})
		end
	end
end

-- Whether a user message is what Codex put in for the person: its
-- environment, the instructions files, permissions.
local function injected(text)
	text = trim_start(text)
	if starts(text, "# AGENTS.md") then
		return true
	end
	if not starts(text, "<") then
		return false
	end
	local name = string.match(string.sub(text, 2), "^([^>]*)")
	return name ~= "" and string.match(name, "^[a-z_]+$") ~= nil
end

-- A session from `offset` on, as text for the model.
local function read_codex_chat(files, item, offset)
	local lines, after = read.lines_from(files, item.path, offset, item.size)
	local out = {}
	local events = 0
	for _, line in ipairs(read.lines(lines)) do
		local event = decode(line)
		if event ~= nil then
			events += 1
			if field(event, "type") == "response_item" then
				local payload = field(event, "payload")
				local kind = field(payload, "type")
				if kind == "message" then
					local role = field(payload, "role")
					local who = (role == "user" or role == "assistant") and role or nil
					local content = field(payload, "content")
					if who ~= nil and is_array(content) then
						for _, part in ipairs(content) do
							local said = str(field(part, "text")) or ""
							if not (who == "user" and injected(said)) then
								said_by(out, who, said)
							end
						end
					end
				elseif kind == "function_call" or kind == "custom_tool_call" then
					local args = kind == "function_call" and field(payload, "arguments") or field(payload, "input")
					local name = str(field(payload, "name")) or "tool"
					table.insert(out, "[call " .. name .. "] " .. read.cut(str(args) or "", CALL_CHARS) .. "\n\n")
				elseif kind == "function_call_output" or kind == "custom_tool_call_output" then
					local output = field(payload, "output")
					local text = str(output)
					if text == nil then
						local raw = ordered.get(ordered.get(ordered.parse(line), "payload"), "output")
						text = raw and ordered.text(raw) or "null"
					end
					table.insert(out, "[result]\n" .. read.cut(read.trim(text), RESULT_CHARS) .. "\n\n")
				end
			end
		end
	end
	if events == 0 and read.trim(lines) ~= "" then
		return nil, "not a Codex session"
	end
	return { text = table.concat(out), offset = after }
end

-- Reading -----------------------------------------------------------------

-- `tool`'s folder under `home`: `~/.claude`, `~/.codex`.
function read.root(home, tool)
	return join(home, tool == read.CLAUDE and ".claude" or ".codex")
end

-- Every item both tools have under `home`: `{ items, failed }`, `failed`
-- the paths that did not read.
function read.scan(files, home)
	local scan = { items = {}, failed = {} }
	local place_of = read.remotes(files)
	scan_claude(files, read.root(home, read.CLAUDE), place_of, scan)
	scan_codex(files, read.root(home, read.CODEX), place_of, scan)
	return scan
end

-- Reads `item`, from `offset` for a chat: `{ note }`, `{ text, offset }`,
-- or nil and why.
function read.item(files, item, offset)
	if item.tool == read.CLAUDE and item.material == read.SAVED then
		local text, why = text_of(files, item.path)
		if text == nil then
			return nil, why
		end
		local note, error = read.parse_note(text, item.path)
		if note == nil then
			return nil, error
		end
		return { note = note }
	elseif item.material == read.CHAT then
		local ok, found, why = pcall(item.tool == read.CLAUDE and read_claude_chat or read_codex_chat, files, item, offset)
		if not ok then
			return nil, tostring(found)
		end
		return found, why
	end
	local text = item.section
	if text == nil then
		local why
		text, why = text_of(files, item.path)
		if text == nil then
			return nil, why
		end
	end
	return { text = text, offset = #text }
end

return read
