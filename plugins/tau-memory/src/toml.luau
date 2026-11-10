-- The TOML of a note's front matter. Strings are written as the toml
-- crate (1.1, `toml_writer`) writes them, byte for byte, so a note
-- written here reads the same as one tau wrote before; reading takes
-- TOML 1.1 whole, comments, inline tables and every string kind
-- included, as a person editing a note might write it.
local M = {}

-- The bytes of `s` that decide how it is quoted.
local function metrics(s)
	local m = { single = 0, double = 0, escape = false, newline = false, codes = false }
	local singles, doubles = 0, 0
	for i = 1, #s do
		local b = string.byte(s, i)
		if b == 39 then
			singles += 1
			m.single = math.max(m.single, singles)
		else
			singles = 0
		end
		if b == 34 then
			doubles += 1
			m.double = math.max(m.double, doubles)
		else
			doubles = 0
		end
		if b == 92 then
			m.escape = true
		elseif b == 9 then
			-- A tab is always allowed.
		elseif b == 10 then
			m.newline = true
		elseif b <= 0x1f or b == 0x7f then
			m.codes = true
		end
	end
	return m
end

local ESCAPES = { [8] = "\\b", [9] = "\\t", [12] = "\\f", [13] = "\\r", [92] = "\\\\" }

-- `s` in a basic string's escapes; `ml` for a multi-line one.
local function escaped(s, ml)
	local out = {}
	local most = ml and 2 or 0
	local at = 1
	while at <= #s do
		local start = at
		local doubles = 0
		local escape
		local stop = at
		while stop <= #s do
			local b = string.byte(s, stop)
			if b == 34 then
				doubles += 1
				if doubles > most then
					escape = '\\"'
					break
				end
			else
				doubles = 0
			end
			if ESCAPES[b] then
				escape = ESCAPES[b]
				break
			elseif b == 10 then
				if not ml then
					escape = "\\n"
					break
				end
			elseif b ~= 34 and (b <= 0x1f or b == 0x7f) then
				break
			end
			stop += 1
		end
		table.insert(out, string.sub(s, start, stop - 1))
		if escape then
			table.insert(out, escape)
			at = stop + 1
		elseif stop <= #s then
			table.insert(out, string.format("\\u%04X", string.byte(s, stop)))
			at = stop + 1
		else
			at = stop
		end
	end
	return table.concat(out)
end

-- `s` as a TOML string, quoted as `toml_writer`'s `as_default` picks.
function M.string(s)
	local m = metrics(s)
	if not m.codes and not m.escape and m.double == 0 and not m.newline then
		return '"' .. escaped(s, false) .. '"'
	end
	if not m.codes and m.single == 0 and not m.newline then
		return "'" .. s .. "'"
	end
	local lead = m.newline and "\n" or ""
	if not m.codes and not m.escape and m.double <= 2 then
		return '"""' .. lead .. escaped(s, true) .. '"""'
	end
	if not m.codes and m.single <= 2 then
		return "'''" .. lead .. s .. "'''"
	end
	if m.newline then
		return '"""' .. lead .. escaped(s, true) .. '"""'
	end
	return '"' .. escaped(s, false) .. '"'
end

-- A whole number as TOML writes it.
function M.integer(n)
	return string.format("%d", n)
end

-- A list of strings, inline.
function M.strings(list)
	local parts = {}
	for _, s in ipairs(list) do
		table.insert(parts, M.string(s))
	end
	return "[" .. table.concat(parts, ", ") .. "]"
end

-- Reading.

local Parser = {}
Parser.__index = Parser

local function fail(p, why)
	-- The line the parser stopped on, from 1.
	local line = 1
	for _ in string.gmatch(string.sub(p.text, 1, p.at - 1), "\n") do
		line += 1
	end
	error({ toml = why .. " at line " .. line }, 0)
end

function Parser:peek(n)
	return string.sub(self.text, self.at, self.at + (n or 1) - 1)
end

function Parser:eof()
	return self.at > #self.text
end

-- Spaces and tabs.
function Parser:blank()
	while true do
		local c = self:peek()
		if c == " " or c == "\t" then
			self.at += 1
		else
			return
		end
	end
end

function Parser:comment()
	if self:peek() == "#" then
		while not self:eof() and self:peek() ~= "\n" and self:peek(2) ~= "\r\n" do
			local b = string.byte(self.text, self.at)
			if (b < 32 and b ~= 9) or b == 127 then
				fail(self, "a control character in a comment")
			end
			self.at += 1
		end
	end
end

-- The end of a line, or of the text.
function Parser:newline()
	if self:peek() == "\n" then
		self.at += 1
		return true
	elseif self:peek(2) == "\r\n" then
		self.at += 2
		return true
	end
	return false
end

-- Blank lines and comments, inside an array or an inline table.
function Parser:gaps()
	while true do
		self:blank()
		self:comment()
		if not self:newline() then
			return
		end
	end
end

local function hex_char(p, digits)
	local hex = p:peek(digits)
	if #hex ~= digits or not string.match(hex, "^%x+$") then
		fail(p, "a bad escape")
	end
	p.at += digits
	local code = tonumber(hex, 16)
	if (code >= 0xD800 and code <= 0xDFFF) or code > 0x10FFFF then
		fail(p, "an escape that is no character")
	end
	return utf8.char(code)
end

-- The text of a string up to `close`, its escapes read when `basic`.
function Parser:string_body(close, basic, multi)
	local out = {}
	if multi and not self:newline() then
		-- A newline right after the opening delimiter is dropped.
	end
	while true do
		if self:eof() then
			fail(self, "a string that never closes")
		end
		if self:peek(#close) == close then
			-- Up to two more quotes may end a multi-line string.
			local extra = 0
			if multi then
				while extra < 2 and string.sub(self.text, self.at + #close + extra, self.at + #close + extra) == string.sub(close, 1, 1) do
					extra += 1
				end
			end
			table.insert(out, string.rep(string.sub(close, 1, 1), extra))
			self.at += #close + extra
			return table.concat(out)
		end
		local c = self:peek()
		local b = string.byte(c)
		if basic and c == "\\" then
			self.at += 1
			local e = self:peek()
			self.at += 1
			if e == "b" then
				table.insert(out, "\b")
			elseif e == "t" then
				table.insert(out, "\t")
			elseif e == "n" then
				table.insert(out, "\n")
			elseif e == "f" then
				table.insert(out, "\f")
			elseif e == "r" then
				table.insert(out, "\r")
			elseif e == "e" then
				table.insert(out, "\27")
			elseif e == '"' then
				table.insert(out, '"')
			elseif e == "\\" then
				table.insert(out, "\\")
			elseif e == "x" then
				table.insert(out, hex_char(self, 2))
			elseif e == "u" then
				table.insert(out, hex_char(self, 4))
			elseif e == "U" then
				table.insert(out, hex_char(self, 8))
			elseif multi and (e == " " or e == "\t" or e == "\n" or e == "\r") then
				-- A line-ending backslash: the white space after it goes.
				self.at -= 1
				self:blank()
				if not self:newline() then
					fail(self, "a backslash before a space that ends no line")
				end
				while true do
					self:blank()
					if not self:newline() then
						break
					end
				end
			else
				fail(self, "a bad escape")
			end
		elseif c == "\n" or self:peek(2) == "\r\n" then
			if not multi then
				fail(self, "a line break in a one-line string")
			end
			table.insert(out, "\n")
			self.at += c == "\n" and 1 or 2
		elseif (b < 32 and b ~= 9) or b == 127 then
			fail(self, "a control character in a string")
		else
			table.insert(out, c)
			self.at += 1
		end
	end
end

function Parser:quoted()
	if self:peek(3) == '"""' then
		self.at += 3
		self:newline()
		return self:string_body('"""', true, true)
	elseif self:peek(3) == "'''" then
		self.at += 3
		self:newline()
		return self:string_body("'''", false, true)
	elseif self:peek() == '"' then
		self.at += 1
		return self:string_body('"', true, false)
	elseif self:peek() == "'" then
		self.at += 1
		return self:string_body("'", false, false)
	end
	return nil
end

-- One key, bare or quoted.
function Parser:simple_key()
	local c = self:peek()
	if c == '"' or c == "'" then
		if self:peek(3) == '"""' or self:peek(3) == "'''" then
			fail(self, "a multi-line key")
		end
		return self:quoted()
	end
	local key = string.match(self.text, "^[%w_%-]+", self.at)
	if key == nil then
		fail(self, "a key expected")
	end
	self.at += #key
	return key
end

-- A key, dotted or not, as its parts.
function Parser:key()
	local parts = {}
	while true do
		self:blank()
		table.insert(parts, self:simple_key())
		self:blank()
		if self:peek() == "." then
			self.at += 1
		else
			return parts
		end
	end
end

-- A value that is not a string, array nor table: a number, a boolean, or
-- a date or time, kept as its text.
function Parser:scalar()
	local word = string.match(self.text, "^[%w_%+%-%.:]+", self.at)
	if word == nil then
		fail(self, "a value expected")
	end
	-- A date may have a time after one space.
	if string.match(word, "^%d%d%d%d%-%d%d%-%d%d$") then
		local time = string.match(self.text, "^ %d%d:%d%d[%w_%+%-%.:]*", self.at + #word)
		if time then
			word ..= time
		end
	end
	self.at += #word
	if word == "true" then
		return true
	elseif word == "false" then
		return false
	end
	local clean = string.gsub(word, "_", "")
	if string.match(word, "^[%+%-]?%d[%d_]*$") then
		if string.match(word, "__") or string.match(word, "_$") or string.match(word, "^[%+%-]?0%d") then
			fail(self, "a bad integer")
		end
		return { integer = tonumber(clean) }
	end
	for _, prefix in ipairs({ { "0x", 16, "%x" }, { "0o", 8, "[0-7]" }, { "0b", 2, "[01]" } }) do
		if string.sub(word, 1, 2) == prefix[1] then
			local digits = string.sub(clean, 3)
			if digits == "" or not string.match(digits, "^" .. prefix[3] .. "+$") then
				fail(self, "a bad integer")
			end
			return { integer = tonumber(digits, prefix[2]) }
		end
	end
	if string.match(word, "^[%+%-]?inf$") or string.match(word, "^[%+%-]?nan$") then
		return { float = word }
	end
	if string.match(clean, "^[%+%-]?%d+%.?%d*[eE]?[%+%-]?%d*$") and tonumber(clean) then
		return { float = word }
	end
	if string.match(word, "^%d%d%d%d%-%d%d%-%d%d") or string.match(word, "^%d%d:%d%d") then
		return { datetime = word }
	end
	fail(self, "a bad value " .. string.format("%q", word))
	return nil
end

local ARRAY = {}

function Parser:value()
	local c = self:peek()
	if c == '"' or c == "'" then
		return { string = self:quoted() }
	elseif c == "[" then
		self.at += 1
		local items = { [ARRAY] = true }
		while true do
			self:gaps()
			if self:peek() == "]" then
				self.at += 1
				return { array = items }
			end
			table.insert(items, self:value())
			self:gaps()
			if self:peek() == "," then
				self.at += 1
			elseif self:peek() == "]" then
				self.at += 1
				return { array = items }
			else
				fail(self, "an array that never closes")
			end
		end
	elseif c == "{" then
		self.at += 1
		local into = { fields = {}, inline = true }
		while true do
			self:gaps()
			if self:peek() == "}" then
				self.at += 1
				return { table = into }
			end
			local key = self:key()
			self:blank()
			if self:peek() ~= "=" then
				fail(self, "= expected")
			end
			self.at += 1
			self:blank()
			self:assign(into, key, self:value())
			self:gaps()
			if self:peek() == "," then
				self.at += 1
			elseif self:peek() == "}" then
				self.at += 1
				return { table = into }
			else
				fail(self, "an inline table that never closes")
			end
		end
	end
	return self:scalar()
end

-- A table under `into` by `name`, made when missing.
local function sub_table(p, into, name, defining)
	local found = into.fields[name]
	if found == nil then
		found = { table = { fields = {}, implicit = not defining } }
		into.fields[name] = found
		return found.table
	end
	if found.table then
		if found.table.inline then
			fail(p, "a table defined inline extended")
		end
		return found.table
	end
	if found.tables then
		return found.tables[#found.tables]
	end
	fail(p, "a key defined twice: " .. name)
	return nil
end

function Parser:assign(into, key, value)
	for i = 1, #key - 1 do
		local t = sub_table(self, into, key[i], false)
		into = t
	end
	local last = key[#key]
	if into.fields[last] ~= nil then
		fail(self, "a key defined twice: " .. last)
	end
	into.fields[last] = value
end

-- The front matter `text` as a tree of `{ string | integer | float |
-- datetime | array | table }` values; or nil and why it did not read.
function M.parse(text)
	local p = setmetatable({ text = text, at = 1 }, Parser)
	local root = { fields = {} }
	local current = root
	local defined = {}
	local ok, err = pcall(function()
		while not p:eof() do
			p:blank()
			p:comment()
			if p:newline() then
				continue
			end
			if p:eof() then
				break
			end
			if p:peek(2) == "[[" then
				p.at += 2
				local key = p:key()
				if p:peek(2) ~= "]]" then
					fail(p, "]] expected")
				end
				p.at += 2
				local into = root
				for i = 1, #key - 1 do
					into = sub_table(p, into, key[i], false)
				end
				local last = key[#key]
				local found = into.fields[last]
				if found == nil then
					found = { tables = {} }
					into.fields[last] = found
				elseif found.tables == nil then
					fail(p, "a key defined twice: " .. last)
				end
				current = { fields = {} }
				table.insert(found.tables, current)
			elseif p:peek() == "[" then
				p.at += 1
				local key = p:key()
				if p:peek() ~= "]" then
					fail(p, "] expected")
				end
				p.at += 1
				local name = table.concat(key, "\0")
				if defined[name] then
					fail(p, "a table defined twice")
				end
				defined[name] = true
				local into = root
				for i = 1, #key do
					into = sub_table(p, into, key[i], i == #key)
				end
				current = into
			else
				local key = p:key()
				if p:peek() ~= "=" then
					fail(p, "= expected")
				end
				p.at += 1
				p:blank()
				p:assign(current, key, p:value())
			end
			p:blank()
			p:comment()
			if not p:newline() and not p:eof() then
				fail(p, "a line that goes on after its value")
			end
		end
	end)
	if not ok then
		if type(err) == "table" and err.toml then
			return nil, err.toml
		end
		error(err, 0)
	end
	return root
end

M.ARRAY = ARRAY

return M
