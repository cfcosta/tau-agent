-- What never reaches a note: secrets, which are redacted, and text that
-- reads as instructions to the model, which is refused. Memory is data;
-- it is fenced as untrusted when read back, and checked here when
-- written (`docs/research/memory.md`, "Memory is data, not
-- instructions").
--
-- The patterns are the regular expressions the Rust tau-memory used,
-- each written as a tree for a small backtracking matcher that reads
-- them as the regex crate does: leftmost first, `\b` and `\s` by
-- Unicode, and `(?i)` folding the Kelvin sign into `k` and the long s
-- into `s`, as Unicode's case folding does.
local text = require("text")
local unicode = require("unicode")

local M = {}

-- What a redacted secret becomes.
M.REDACTED = "[redacted]"

-- Pattern pieces.

local function lit(s, ci)
	return { lit = text.codes(s), ci = ci }
end

local function class(test)
	return { class = test }
end

local function seq(...)
	return { seq = { ... } }
end

local function alt(...)
	return { alt = { ... } }
end

local function rep(node, min, max, lazy)
	return { rep = node, min = min, max = max, lazy = lazy }
end

local WB = { wb = true }

local function is_word(code)
	if code == nil then
		return false
	end
	if code < 128 then
		return (code >= 48 and code <= 57) or (code >= 65 and code <= 90) or (code >= 97 and code <= 122) or code == 95
	end
	return unicode.has(unicode.WORD, code)
end

local is_space = text.is_space

-- The ASCII letter `code` is under `(?i)`, lowercase, or nil.
local function folded(code)
	if code >= 65 and code <= 90 then
		return code + 32
	elseif code >= 97 and code <= 122 then
		return code
	end
	local letter = unicode.FOLD[code]
	return letter and string.byte(letter) or nil
end

-- A class of ASCII ranges, `spec` as a regex writes it inside `[]`;
-- `ci` lets letters match as `(?i)` does.
local function set(spec, ci)
	local allowed = {}
	local i = 1
	while i <= #spec do
		local c = string.sub(spec, i, i)
		if c == "\\" then
			c = string.sub(spec, i + 1, i + 1)
			i += 1
		end
		local lo = string.byte(c)
		local hi = lo
		if string.sub(spec, i + 1, i + 1) == "-" and i + 2 <= #spec then
			hi = string.byte(spec, i + 2)
			i += 2
		end
		for code = lo, hi do
			allowed[code] = true
		end
		i += 1
	end
	return class(function(code)
		if allowed[code] then
			return true
		end
		if ci then
			local letter = folded(code)
			return letter ~= nil and (allowed[letter] or allowed[letter - 32]) == true
		end
		return false
	end)
end

local SPACE = class(is_space)
local ANY = class(function()
	return true
end)

local function spaces(min)
	return rep(SPACE, min, nil)
end

-- Matching: `node` at `at` in `codes`, then `k` from where it ended.

local single

local function match(node, codes, at, k)
	if node.lit then
		local want = node.lit
		for i, code in ipairs(want) do
			local got = codes[at + i - 1]
			if got == nil then
				return nil
			end
			if got ~= code then
				if not (node.ci and folded(code) ~= nil and folded(got) == folded(code)) then
					return nil
				end
			end
		end
		return k(at + #want)
	elseif node.class then
		local got = codes[at]
		if got ~= nil and node.class(got) then
			return k(at + 1)
		end
		return nil
	elseif node.wb then
		if is_word(codes[at - 1]) ~= is_word(codes[at]) then
			return k(at)
		end
		return nil
	elseif node.seq then
		local parts = node.seq
		local function from(i, pos)
			if i > #parts then
				return k(pos)
			end
			return match(parts[i], codes, pos, function(next_at)
				return from(i + 1, next_at)
			end)
		end
		return from(1, at)
	elseif node.alt then
		for _, option in ipairs(node.alt) do
			local found = match(option, codes, at, k)
			if found then
				return found
			end
		end
		return nil
	elseif node.rep then
		local inner = node.rep
		if single(inner) then
			-- One character each: how far a run goes, then each count
			-- from the most (greedy) or the least (lazy).
			local most = 0
			local limit = node.max or math.huge
			while most < limit and codes[at + most] ~= nil and match(inner, codes, at + most, function(x)
				return x
			end) do
				most += 1
			end
			if most < node.min then
				return nil
			end
			if node.lazy then
				for count = node.min, most do
					local found = k(at + count)
					if found then
						return found
					end
				end
			else
				for count = most, node.min, -1 do
					local found = k(at + count)
					if found then
						return found
					end
				end
			end
			return nil
		end
		-- `(...)?`: a group, once or not.
		assert(node.min == 0 and node.max == 1, "only a single character repeats more")
		local found = match(inner, codes, at, k)
		if found then
			return found
		end
		return k(at)
	end
	error("not a pattern piece")
end

single = function(node)
	return node.class ~= nil or (node.lit ~= nil and #node.lit == 1)
end

-- Where `pattern` first matches in `codes` from `from`: start and end.
local function find(pattern, codes, from)
	for at = from, #codes do
		local stop = match(pattern.tree, codes, at, function(x)
			return x
		end)
		if stop then
			return at, stop
		end
	end
	return nil
end

-- Whether `s` may hold a match: one of the words a match must hold is
-- there. Folding reaches past ASCII, so a text that is not ASCII is
-- always looked through.
local function may_hold(pattern, s, lowered, ascii)
	if pattern.ci and not ascii then
		return true
	end
	for _, word in ipairs(pattern.needs) do
		if pattern.ci then
			if string.find(lowered, word, 1, true) then
				return true
			end
		elseif string.find(s, word, 1, true) then
			return true
		end
	end
	return false
end

local CI = true

local UPPER_SPACE = set("A-Z ")

-- Credentials that have a recognizable shape.
local SECRETS = {
	-- Private keys, whole blocks.
	{
		needs = { "-----BEGIN " },
		tree = seq(lit("-----BEGIN "), rep(UPPER_SPACE, 0), lit("PRIVATE KEY-----"), rep(ANY, 0, nil, true), lit("-----END "), rep(UPPER_SPACE, 0), lit("PRIVATE KEY-----")),
	},
	-- OpenAI and Anthropic style keys.
	{
		needs = { "sk-" },
		tree = seq(WB, lit("sk-"), rep(alt(lit("ant-"), lit("proj-")), 0, 1), rep(set("A-Za-z0-9_\\-"), 20)),
	},
	-- GitHub tokens.
	{ needs = { "gh" }, tree = seq(WB, lit("gh"), set("pousr"), lit("_"), rep(set("A-Za-z0-9"), 30)) },
	{ needs = { "github_pat_" }, tree = seq(WB, lit("github_pat_"), rep(set("A-Za-z0-9_"), 30)) },
	-- AWS access keys.
	{ needs = { "AKIA", "ASIA" }, tree = seq(WB, alt(lit("AKIA"), lit("ASIA")), rep(set("A-Z0-9"), 16, 16), WB) },
	-- Slack tokens.
	{ needs = { "xox" }, tree = seq(WB, lit("xox"), set("abprs"), lit("-"), rep(set("A-Za-z0-9\\-"), 10)) },
	-- Google API keys.
	{ needs = { "AIza" }, tree = seq(WB, lit("AIza"), rep(set("A-Za-z0-9_\\-"), 35, 35)) },
	-- Bearer tokens in headers.
	{
		ci = true,
		needs = { "bearer" },
		tree = seq(WB, lit("bearer", CI), spaces(1), rep(set("A-Za-z0-9_\\-\\.=", CI), 20)),
	},
	-- `password = …`, `api_key: …` and the like, the value only.
	{
		ci = true,
		needs = { "password", "passwd", "secret", "token", "api" },
		tree = seq(
			WB,
			alt(lit("password", CI), lit("passwd", CI), lit("secret", CI), lit("token", CI), seq(lit("api", CI), rep(set("_\\-"), 0, 1), lit("key", CI))),
			WB,
			spaces(0),
			set(":="),
			spaces(0),
			rep(set("'\""), 0, 1),
			rep(
				class(function(code)
					return not is_space(code) and code ~= 39 and code ~= 34
				end),
				8
			)
		),
	},
}

local function words(...)
	local options = {}
	for _, word in ipairs({ ... }) do
		table.insert(options, lit(word, CI))
	end
	return alt(table.unpack(options))
end

-- Phrases that address the model rather than record anything.
local INSTRUCTIONS = {
	{
		ci = true,
		needs = { "ignore" },
		tree = seq(
			WB,
			lit("ignore", CI),
			spaces(1),
			rep(alt(seq(lit("all", CI), spaces(1)), seq(lit("any", CI), spaces(1))), 0, 1),
			rep(seq(lit("the", CI), spaces(1)), 0, 1),
			words("previous", "prior", "above", "earlier"),
			spaces(1),
			words("instructions", "messages", "rules")
		),
	},
	{
		ci = true,
		needs = { "disregard" },
		tree = seq(
			WB,
			lit("disregard", CI),
			spaces(1),
			rep(alt(seq(lit("all", CI), spaces(1)), seq(lit("any", CI), spaces(1))), 0, 1),
			rep(seq(lit("the", CI), spaces(1)), 0, 1),
			words("previous", "prior", "above", "system"),
			WB
		),
	},
	{
		ci = true,
		needs = { "you" },
		tree = seq(WB, lit("you", CI), spaces(1), lit("are", CI), spaces(1), lit("now", CI), spaces(1), words("a", "an", "in"), WB),
	},
	{
		ci = true,
		needs = { "new" },
		tree = seq(WB, lit("new", CI), spaces(1), lit("system", CI), spaces(1), lit("prompt", CI), WB),
	},
	{
		ci = true,
		needs = { "<" },
		tree = seq(lit("<"), rep(lit("|"), 0, 1), spaces(0), words("im_start", "im_end", "system"), spaces(0), rep(lit("|"), 0, 1), lit(">")),
	},
	{
		ci = true,
		needs = { "[" },
		tree = seq(lit("["), spaces(0), lit("system", CI), spaces(0), lit("]"), spaces(0), lit(":")),
	},
}

local function is_ascii(s)
	return not string.find(s, "[\128-\255]")
end

local function encode(codes, from, to)
	local out = {}
	for i = from, to - 1 do
		table.insert(out, utf8.char(codes[i]))
	end
	return table.concat(out)
end

-- `s` with every recognizable secret replaced by the redaction mark, and
-- how many were.
function M.redact(s)
	local count = 0
	for _, pattern in ipairs(SECRETS) do
		local ascii = is_ascii(s)
		if may_hold(pattern, s, string.lower(s), ascii) then
			local codes = text.codes(s)
			local out = {}
			local at = 1
			local found = 0
			while true do
				local start, stop = find(pattern, codes, at)
				if start == nil then
					break
				end
				table.insert(out, encode(codes, at, start))
				table.insert(out, M.REDACTED)
				found += 1
				at = stop
			end
			if found > 0 then
				table.insert(out, encode(codes, at, #codes + 1))
				s = table.concat(out)
				count += found
			end
		end
	end
	return s, count
end

-- Zero-width and direction-changing characters, which hide text.
local function is_hidden(code)
	return (code >= 0x200B and code <= 0x200F)
		or (code >= 0x202A and code <= 0x202E)
		or (code >= 0x2060 and code <= 0x2064)
		or (code >= 0x2066 and code <= 0x2069)
		or code == 0xFEFF
end

-- Why `s` cannot be stored, when it reads as instructions to the model
-- or hides characters a reader would not see; nil when it can.
function M.refusal(s)
	if not is_ascii(s) then
		for _, code in utf8.codes(s) do
			if is_hidden(code) then
				return string.format("it holds an invisible character (U+%04X); write plain text", code)
			end
		end
	end
	local ascii = is_ascii(s)
	local lowered = string.lower(s)
	local codes
	for _, pattern in ipairs(INSTRUCTIONS) do
		if may_hold(pattern, s, lowered, ascii) then
			codes = codes or text.codes(s)
			local start, stop = find(pattern, codes, 1)
			if start then
				return string.format('"%s" reads as an instruction to the model; memory records what is, not what to do', encode(codes, start, stop))
			end
		end
	end
	return nil
end

return M
