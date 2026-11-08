-- Building lines with a model (docs/reference/tree-compaction.md,
-- "Building lines"), against a stand-in that answers by what it is
-- asked: growing builds every line within the budget, asking only for
-- lines that need a model; no request shows a label; a long line goes
-- back cut at the limit; a failure fails the growth; and a line reads
-- what came before it.
local tau = require("tau")
local t = tau.test
local g = t.gen
local tree = require("tree")
local build = require("build")

local N = tree.NODE_BYTES

-- The step a request asks: its task's second block.
local function step(request)
	return request.task[2]
end

local function last_line(text)
	local lines = string.split(text, "\n")
	return lines[#lines]
end

-- A line of `bytes` bytes for a request, tagged by its step so lines
-- differ.
local function line_of(request, bytes)
	local line = "s:" .. #last_line(step(request))
	if #line < bytes then
		line ..= string.rep("y", bytes - #line)
	end
	return string.sub(line, 1, bytes)
end

-- A stand-in model answering each request with `answer`, keeping them.
local function fake(answer)
	local asked = {}
	return function(request)
		table.insert(asked, request)
		local text, why = answer(request, #asked)
		if text == nil then
			return { ok = false, error = why, reason = "error" }
		end
		return { ok = true, value = text }
	end, asked
end

local function grown(entries, budget, ask, jobs)
	local history = tree.new()
	for _, entry in ipairs(entries) do
		tree.push(history, entry)
	end
	local result, why = build.grow(history, budget, ask, { jobs = jobs or 8 })
	return history, result, why
end

-- An entry's text: words with no digits, `+` or `|`, so a label in a
-- request can only come from the request itself.
local entry = g.composite(function(draw)
	local words = {}
	for w = 1, draw(g.integers({ min = 1, max = 400 })) do
		words[w] = ({ "alpha", "beta", "gamma" })[w % 3 + 1]
	end
	return { kind = draw(g.sampled_from({ "user", "talk", "tool", "echo" })), text = table.concat(words, " ") }
end)

t.property("growing builds every line within budget, asking only when needed", function(draw)
	local entries = draw(g.lists(entry, { min = 16, max = 80 }))
	local bits, left = 0, #entries
	while left > 0 do
		bits += left % 2
		left //= 2
	end
	local budget = draw(g.integers({ min = bits, max = bits + 20 })) * N
	local bytes = draw(g.integers({ min = 100, max = N }))
	local ask, asked = fake(function(request)
		return line_of(request, bytes)
	end)
	local history, result = grown(entries, budget, ask)
	t.equal(result ~= nil, true)
	for _, node in ipairs(history.view) do
		t.equal(tree.is_built(history, node), true)
	end
	t.equal(tree.view_bytes(history, 0) <= budget, true)
	t.equal(result.asked, #asked)
	-- Exactly the long entries were compressed by a model; a short one is
	-- its own line, word for word.
	local compressed, long = 0, 0
	for _, request in ipairs(asked) do
		if string.find(step(request), "Compress this message", 1, true) then
			compressed += 1
		end
	end
	for at, one in ipairs(entries) do
		if #tree.line(one) > N then
			long += 1
		else
			t.equal(tree.text(history, tree.leaf(at - 1)), tree.line(one))
		end
	end
	t.equal(compressed, long)
	for _, request in ipairs(asked) do
		local text = step(request)
		-- A merge asks only when its two lines do not fit together.
		if string.find(text, "Merge these", 1, true) then
			local lines = string.split(text, "\n")
			t.equal(#lines[#lines] + 1 + #lines[#lines - 1] > N, true)
		end
		t.equal(request.instructions, build.COMPACT_PROMPT)
		t.equal(#request.task, 2, "context, then the step")
		t.equal(string.sub(request.task[1], 1, 6), "<chat>")
	end
end, { cases = 30 })

t.property("no request shows a label", function(draw)
	local entries = draw(g.lists(entry, { min = 2, max = 60 }))
	local ask, asked = fake(function(request)
		return line_of(request, 300)
	end)
	-- Folded in two batches, so the second's requests read the first's
	-- view as context.
	local history = tree.new()
	local half = #entries // 2
	for at = 1, half do
		tree.push(history, entries[at])
	end
	build.grow(history, 4000, ask, { jobs = 8 })
	for at = half + 1, #entries do
		tree.push(history, entries[at])
	end
	build.grow(history, 4000, ask, { jobs = 8 })
	for _, request in ipairs(asked) do
		for _, text in ipairs(request.task) do
			t.equal(string.find(text, "%d+%+%d+|"), nil, "a label in:\n" .. text)
		end
	end
end, { cases = 40 })

t.property("cutting at a byte limit never splits a character", function(draw)
	local text = draw(g.text({ max = 200 }))
	local bytes = draw(g.integers({ min = 0, max = 900 }))
	local cut = build.cut_bytes(text, bytes)
	t.equal(string.sub(text, 1, #cut), cut)
	t.equal(#cut <= bytes, true)
	-- The largest such prefix: the next character would pass the limit.
	if #cut < #text then
		local next = utf8.offset(text, 2, #cut + 1)
		t.equal(next - 1 > bytes, true)
	end
end)

t.case("a long line goes back cut at the limit, and the shortest is kept", function()
	local sizes = { 600, 550, 530, 520, 515, 513 }
	local ask, asked = fake(function(_, n)
		return string.rep("w", sizes[n])
	end)
	local history = grown({ { kind = "echo", text = string.rep("v", 2 * N) } }, 10000, ask)
	t.equal(#asked, build.TRIES)
	t.equal(#tree.text(history, tree.leaf(0)), 515)
	local last = asked[#asked]
	-- The first message, then each try and what it was told.
	t.equal(#last.messages, 2 * (build.TRIES - 1))
	t.equal(last.messages[1].role, "user")
	t.equal(last.messages[2], { role = "assistant", text = string.rep("w", 600) })
	t.equal(
		last.messages[3].text,
		"That line is 600 bytes; the limit is "
			.. N
			.. ". It must end where it is cut here:\n"
			.. string.rep("w", N)
			.. "| ← LIMIT"
	)
end)

t.case("a line that fits stops the tries", function()
	local ask, asked = fake(function(_, n)
		return string.rep("w", n == 1 and 700 or 300)
	end)
	local history = grown({ { kind = "echo", text = string.rep("v", 2 * N) } }, 10000, ask)
	t.equal(#asked, 2)
	t.equal(#tree.text(history, tree.leaf(0)), 300)
end)

t.case("a failed request, or an empty answer, fails the growth, naming the line", function()
	local failing = fake(function()
		return nil, "overloaded"
	end)
	local _, result, why = grown({ { kind = "echo", text = string.rep("v", 2 * N) } }, 10000, failing)
	t.equal(result, nil)
	t.equal(why, "the compactor failed on 0+1: overloaded")
	local blank = fake(function()
		return "  "
	end)
	_, result, why = grown({ { kind = "echo", text = string.rep("v", 2 * N) } }, 10000, blank)
	t.equal(why, "the compactor gave 0+1 an empty line")
end)

t.case("a view that cannot fit says so", function()
	local ask = fake(function(request)
		return line_of(request, N)
	end)
	local entries = {}
	for at = 1, 3 do
		entries[at] = { kind = "echo", text = string.rep("v", 2 * N) }
	end
	local _, result, why = grown(entries, 10, ask)
	t.equal(result, nil)
	t.equal(string.find(why, "over its 10, and nothing is left to merge", 1, true) ~= nil, true, why)
end)

t.case("a message is compressed with what came before it", function()
	local ask, asked = fake(function(request)
		return line_of(request, 200)
	end)
	local history = tree.new()
	tree.push(history, { kind = "user", text = "fix the parser" })
	build.grow(history, 10000, ask, { jobs = 8 })
	local read = "read src/parse.rs " .. string.rep("p", N)
	tree.push(history, { kind = "tool", text = read })
	tree.push(history, { kind = "echo", text = string.rep("q", 2 * N) })
	tree.push(history, { kind = "talk", text = "the parser bails on a closer" })
	tree.push(history, { kind = "echo", text = string.rep("r", 2 * N) })
	build.grow(history, 10000, ask, { jobs = 8 })
	t.equal(#asked, 3)
	local function context(message)
		for _, request in ipairs(asked) do
			if string.sub(step(request), -#message) == message then
				return request.task[1]
			end
		end
		error("no request for " .. message)
	end
	t.equal(context(read), "<chat>\nuser: fix the parser\n</chat>")
	t.equal(
		context(string.rep("q", 2 * N)),
		"<chat>\nuser: fix the parser\n</chat>\n<recent>\ntool: " .. read .. "\n</recent>"
	)
	-- The talk line is built at once, needing no model; it still reads as
	-- a message, after the tool line still being built.
	local last = context(string.rep("r", 2 * N))
	local head = "<chat>\nuser: fix the parser\n</chat>\n<recent>\ntool: read"
	t.equal(string.sub(last, 1, #head), head)
	local tail = "talk: the parser bails on a closer\n</recent>"
	t.equal(string.sub(last, -#tail), tail)
	local scale = "For scale, this line is exactly " .. N .. " bytes:\n" .. build.SCALE .. "\n\nCompress this message"
	t.equal(string.sub(step(asked[1]), 1, #scale), scale)
end)

t.case("recent keeps the newest messages that fit in its 6,000 characters", function()
	t.equal(build.RECENT_CHARS, 6000)
	local ask, asked = fake(function(request)
		return line_of(request, 200)
	end)
	local history = tree.new()
	for n = 0, 9 do
		local tag = "m" .. n .. " "
		tree.push(history, { kind = "echo", text = tag .. string.rep("k", 1000 - 6 - #tag) })
	end
	local last = string.rep("z", 2 * N)
	tree.push(history, { kind = "echo", text = last })
	build.grow(history, 1000000, ask, { jobs = 8 })
	local context
	for _, request in ipairs(asked) do
		if string.sub(step(request), -#last) == last then
			context = request.task[1]
		end
	end
	local recent = string.match(context, "<recent>\n(.*)\n</recent>$")
	local tags = {}
	for _, line in ipairs(string.split(recent, "\n")) do
		table.insert(tags, string.split(line, " ")[2])
		t.equal(utf8.len(line), 1000)
	end
	t.equal(tags, { "m4", "m5", "m6", "m7", "m8", "m9" })
end)

t.case("the scale line is exactly a line long, on one line", function()
	t.equal(#build.SCALE, N)
	t.equal(string.find(build.SCALE, "\n", 1, true), nil)
end)
