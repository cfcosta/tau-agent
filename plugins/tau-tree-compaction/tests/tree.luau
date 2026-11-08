-- The history, the tree and the view (docs/reference/tree-compaction.md),
-- without a model: names, fitting, what to build, zoom, render, restore,
-- entries and the cap.
local tau = require("tau")
local t = tau.test
local g = t.gen
local tree = require("tree")

local N = tree.NODE_BYTES

-- A text of `bytes` ASCII bytes, starting with `tag` so lines differ.
local function sized(tag, bytes)
	local text = tag .. ":"
	if #text < bytes then
		text ..= string.rep("x", bytes - #text)
	end
	return string.sub(text, 1, math.max(bytes, 1))
end

-- A history of `#lengths` entries whose leaves are built, a leaf's line
-- `lengths[i]` bytes.
local function leaves(lengths)
	local history = tree.new()
	for at, bytes in ipairs(lengths) do
		local id = at - 1
		tree.push(history, { kind = "echo", text = sized("e" .. id, 2000) })
		tree.set(history, tree.leaf(id), sized("l" .. id, bytes))
	end
	return history
end

-- Every parent over `history`'s entries that `build` gives a size, built
-- bottom up, `build` drawing its size from its children's.
local function build_parents(history, build)
	local total = #history.entries
	local level = 1
	while 2 ^ level <= total do
		for index = 0, total // 2 ^ level - 1 do
			local node = tree.node(level, index)
			local a, b = tree.children(node)
			local first, second = tree.text(history, a), tree.text(history, b)
			if first ~= nil and second ~= nil then
				local bytes = build(node, #first, #second)
				if bytes ~= nil then
					tree.set(history, node, sized(tree.label(node), bytes))
				end
			end
		end
		level += 1
	end
end

-- Whether `view` tiles `[0, total)` in order.
local function tiles(view, total)
	local at = 0
	for _, node in ipairs(view) do
		if tree.first(node) ~= at then
			return false
		end
		at = tree.ending(node)
	end
	return at == total
end

-- Whether some two siblings sit side by side with their parent built.
local function mergeable(history)
	local view = history.view
	for at = 1, #view - 1 do
		local a, b = view[at], view[at + 1]
		if a.level == b.level and a.index % 2 == 0 and b.index == a.index + 1 and tree.is_built(history, tree.parent(a)) then
			return true
		end
	end
	return false
end

local function lengths(most)
	return g.lists(g.integers({ min = 1, max = N }), { min = most // 8 + 1, max = most })
end

local function fitting(a, b)
	return math.min(a + 1 + b, N)
end

t.property("node names round trip, and children tile their parent", function(draw)
	local level = draw(g.integers({ min = 0, max = 12 }))
	local index = draw(g.integers({ min = 0, max = 10000 }))
	local node = tree.node(level, index)
	t.equal(tree.named(tree.first(node), tree.count(node)), node)
	t.equal(tree.label(node), tree.first(node) .. "+" .. tree.count(node))
	local a, b = tree.children(node)
	if a == nil then
		t.equal(level, 0)
	else
		t.equal({ tree.first(a), tree.ending(a), tree.ending(b) }, { tree.first(node), tree.first(b), tree.ending(node) })
		t.equal({ tree.parent(a), tree.parent(b) }, { node, node })
	end
	-- A count that is not a power of two, or an id it does not divide,
	-- names nothing.
	local n = draw(g.integers({ min = 2, max = 4096 }))
	local id = draw(g.integers({ min = 0, max = 65536 }))
	local power = n
	while power % 2 == 0 do
		power //= 2
	end
	t.equal(tree.named(id, n) ~= nil, power == 1 and id % n == 0)
end)

t.property("fit tiles the history within budget, or waits for a line", function(draw)
	local history = leaves(draw(lengths(300)))
	local built = draw(g.lists(g.booleans(), { min = 600, max = 600 }))
	local n = 0
	build_parents(history, function(_, a, b)
		n += 1
		if built[n % #built + 1] then
			return fitting(a, b)
		end
		return nil
	end)
	local budget = draw(g.integers({ min = 0, max = 80000 }))
	local fit = tree.fit(history, budget)
	t.equal(tiles(history.view, #history.entries), true)
	if fit == "fits" then
		t.equal(tree.view_bytes(history, N) <= budget, true)
	else
		t.equal(fit, "waiting")
		t.equal(tree.view_bytes(history, N) > budget, true)
		t.equal(mergeable(history), false)
	end
end)

t.property("appending and fitting never splits a line", function(draw)
	local sizes = draw(lengths(200))
	local budget = draw(g.integers({ min = N, max = 20000 }))
	local history = tree.new()
	for at, bytes in ipairs(sizes) do
		local id = at - 1
		local before = tree.copy(history.view)
		tree.push(history, { kind = "user", text = "m" .. id })
		tree.set(history, tree.leaf(id), sized("l" .. id, bytes))
		build_parents(history, function(_, a, b)
			return fitting(a, b)
		end)
		tree.fit(history, budget)
		t.equal(tiles(history.view, id + 1), true)
		for _, old in ipairs(before) do
			local kept = false
			for _, new in ipairs(history.view) do
				kept = kept or (tree.first(new) <= tree.first(old) and tree.ending(old) <= tree.ending(new))
			end
			t.equal(kept, true, tree.label(old) .. " was split")
		end
	end
end, { cases = 60 })

t.property("building the wanted lines makes the view fit", function(draw)
	local sizes = draw(lengths(400))
	local history = leaves(sizes)
	-- Enough for the coarsest view: one line per set bit of the count.
	local bits, left = 0, #sizes
	while left > 0 do
		bits += left % 2
		left //= 2
	end
	local budget = draw(g.integers({ min = N * bits, max = 200000 }))
	local wanted = tree.wanted(history, budget)
	for _, node in ipairs(wanted) do
		local a, b = tree.children(node)
		tree.set(history, node, sized(tree.label(node), fitting(#tree.text(history, a), #tree.text(history, b))))
	end
	t.equal(tree.fit(history, budget), "fits")
	for _, node in ipairs(wanted) do
		local used = false
		for _, line in ipairs(history.view) do
			used = used or (tree.first(line) <= tree.first(node) and tree.ending(node) <= tree.ending(line))
		end
		t.equal(used, true, tree.label(node) .. " was built for nothing")
	end
	for _, node in ipairs(history.view) do
		local merged = node.level == 0
		for _, want in ipairs(wanted) do
			merged = merged or tree.same(want, node)
		end
		t.equal(merged, true, "only what was wanted was merged")
	end
end)

t.property("zoom reaches every message under a line", function(draw)
	local history = leaves(draw(lengths(64)))
	build_parents(history, function(_, a, b)
		return fitting(a, b)
	end)
	tree.fit(history, draw(g.integers({ min = 0, max = 8000 })))
	for _, line in ipairs(tree.copy(history.view)) do
		local open = { line }
		local reached = {}
		while #open > 0 do
			local node = table.remove(open)
			local shown = tree.zoom(history, tree.first(node), tree.count(node))
			local a, b = tree.children(node)
			if a == nil then
				t.equal(shown, tree.first(node) .. "+1|" .. tree.line(history.entries[tree.first(node) + 1]))
				table.insert(reached, tree.first(node))
			else
				local first, second = string.match(shown, "^([^\n]*)\n([^\n]*)$")
				t.equal(string.sub(first, 1, #tree.label(a) + 1), tree.label(a) .. "|")
				t.equal(string.sub(second, 1, #tree.label(b) + 1), tree.label(b) .. "|")
				table.insert(open, b)
				table.insert(open, a)
			end
		end
		table.sort(reached)
		local want = {}
		for id = tree.first(line), tree.ending(line) - 1 do
			table.insert(want, id)
		end
		t.equal(reached, want)
	end
	local total = #history.entries
	t.equal({ tree.zoom(history, total, 1) }, { nil, "No line " .. total .. "+1." })
	t.equal({ tree.zoom(history, 0, 3) }, { nil, "No line 0+3." })
	t.equal({ tree.zoom(history, 1, 2) }, { nil, "No line 1+2." })
end)

t.property("render is one labelled line per part", function(draw)
	local history = leaves(draw(lengths(100)))
	tree.set(history, tree.leaf(0), "first\nline  with\tspaces")
	build_parents(history, function(_, a, b)
		return fitting(a, b)
	end)
	tree.fit(history, draw(g.integers({ min = 0, max = 10000 })))
	local rendered = tree.render(history)
	local lines = string.split(rendered, "\n")
	t.equal(#lines, #history.view)
	for at, line in ipairs(lines) do
		local node = history.view[at]
		local prefix = tree.label(node) .. "|"
		t.equal(string.sub(line, 1, #prefix), prefix)
		t.equal(string.sub(line, #prefix + 1), tree.flat(history, node))
	end
	if tree.same(history.view[1], tree.leaf(0)) then
		t.equal(lines[1], "0+1|first line with spaces")
	end
	-- The context a compactor reads has no labels.
	t.equal(string.find(tree.context(history, #history.entries), "+1|", 1, true), nil)
end)

t.property("restore gives back what was kept", function(draw)
	local history = leaves(draw(lengths(100)))
	build_parents(history, function(_, a, b)
		return fitting(a, b)
	end)
	tree.fit(history, draw(g.integers({ min = 0, max = 10000 })))
	local restored = tree.restore(history.entries, history.nodes, history.view)
	t.equal(restored, history)
	-- A view that does not tile what was kept falls back to the leaves.
	local fewer = table.move(history.entries, 1, #history.entries - 1, 1, {})
	local short = tree.restore(fewer, history.nodes, history.view)
	t.equal(tiles(short.view, #short.entries), true)
	for _, node in ipairs(short.view) do
		t.equal(node.level, 0)
	end
end)

t.property("the cap keeps a long text's head and tail", function(draw)
	local cap = tree.ENTRY_CHARS
	local len = draw(g.one_of({ g.sampled_from({ cap, cap + 1 }), g.integers({ min = 0, max = 3 * cap }) }))
	local pattern = draw(g.lists(g.sampled_from({ "a", "z", "\n", "é", "日", "🦀" }), { min = 1, max = 8 }))
	local chars = {}
	for at = 1, len do
		chars[at] = pattern[(at - 1) % #pattern + 1]
	end
	local text = table.concat(chars)
	local want = text
	if len > cap then
		local half = cap // 2
		want = table.concat(chars, "", 1, half)
			.. "\n[… "
			.. (len - 2 * half)
			.. " characters cut …]\n"
			.. table.concat(chars, "", len - half + 1, len)
	end
	t.equal(tree.cap(text), want)
end, { cases = 40 })

local function call(id, name, json_text)
	return { id = id, name = name, json = json_text }
end

t.case("a message splits into entries by kind", function()
	local user = { role = "user", text = "look at this", images = 1, parts = { { text = "look at this" }, { image = true } } }
	t.equal(tree.entries(user), { { kind = "user", text = "look at this\n[image]" } })
	t.equal(tree.entries({ role = "user", text = "  " }), {})
	local reply = {
		role = "assistant",
		text = "reading both",
		thinking = "hmm",
		calls = { call("c1", "read", '{"path":"a.rs"}'), call("c2", "bash", '{"command":"ls"}') },
	}
	t.equal(tree.entries(reply), {
		{ kind = "talk", text = "reading both" },
		{ kind = "tool", text = 'read {"path":"a.rs"}' },
		{ kind = "tool", text = 'bash {"command":"ls"}' },
	})
	local result = { role = "toolResult", text = "no such file", call = "c1", name = "read", ok = false }
	t.equal(tree.entries(result), { { kind = "echo", text = "[read failed] no such file" } })
end)

t.case("a short entry needs no model, nor two lines that fit together", function()
	local history = tree.new()
	tree.push(history, { kind = "user", text = "keep the AST shape" })
	tree.push(history, { kind = "echo", text = string.rep("x", N) })
	t.equal(tree.free(history, tree.leaf(0)), "user: keep the AST shape")
	t.equal(tree.free(history, tree.leaf(1)), nil)
	tree.set(history, tree.leaf(0), "user: keep the AST shape")
	tree.set(history, tree.leaf(1), "echo: 512 x")
	t.equal(tree.free(history, tree.node(1, 0)), "user: keep the AST shape\necho: 512 x")
	tree.set(history, tree.leaf(1), string.rep("y", N))
	t.equal(tree.free(history, tree.node(1, 0)), nil)
end)

t.property("detail fades with age: an older line never covers less", function(draw)
	local count = draw(g.integers({ min = 1, max = 400 }))
	local bytes = draw(g.integers({ min = 40, max = N }))
	local budget = draw(g.integers({ min = N * 10, max = 40000 }))
	local history = tree.new()
	for id = 0, count - 1 do
		tree.push(history, { kind = "user", text = "m" .. id })
		tree.set(history, tree.leaf(id), sized("l" .. id, bytes))
		build_parents(history, function()
			return bytes
		end)
		t.equal(tree.fit(history, budget), "fits")
		for at = 1, #history.view - 1 do
			t.equal(history.view[at].level >= history.view[at + 1].level, true)
		end
	end
end, { cases = 30 })

-- The pair `fit` should merge next in `view`, by OptChat's rule: of two
-- siblings side by side whose parent is built, the most due, a pair at
-- level `l` from entry `start` being due `(total - start) / 2^(l+2)`,
-- compared exactly; ties go to the older pair.
local function most_due_model(history, view, total)
	local best, top_age, top_power
	for at = 1, #view - 1 do
		local a, b = view[at], view[at + 1]
		if a.level == b.level and tree.same(tree.parent(a), tree.parent(b)) and tree.is_built(history, tree.parent(a)) then
			local age, power = total - tree.first(a), a.level + 2
			if best == nil or age * 2 ^ top_power > top_age * 2 ^ power then
				best, top_age, top_power = at, age, power
			end
		end
	end
	return best
end

t.property("each merge takes the most due pair, ties to the older", function(draw)
	local history = leaves(draw(lengths(64)))
	local built = draw(g.lists(g.booleans(), { min = 128, max = 128 }))
	local n = 0
	build_parents(history, function(_, a, b)
		n += 1
		if built[n % #built + 1] then
			return math.max((a + b) // 2, 1)
		end
		return nil
	end)
	local total = #history.entries
	while true do
		local before = tree.copy(history.view)
		local at = most_due_model(history, before, total)
		local fit = tree.fit(history, tree.view_bytes(history, N) - 1)
		if at == nil then
			t.equal(fit, "waiting")
			t.equal(history.view, before)
			break
		end
		local want = tree.copy(before)
		want[at] = tree.parent(before[at])
		table.remove(want, at + 1)
		t.equal(fit, "fits")
		t.equal(history.view, want)
	end
end)

t.case("a kept view whose line was never built starts again from the leaves", function()
	local history = leaves({ 100, 100, 100, 100 })
	build_parents(history, function()
		return 100
	end)
	history.nodes[tree.key(tree.node(1, 1))] = nil
	local restored = tree.restore(history.entries, history.nodes, { tree.node(1, 0), tree.node(1, 1) })
	t.equal(restored.view, { tree.leaf(0), tree.leaf(1), tree.leaf(2), tree.leaf(3) })
end)

t.case("a line is ready when its sources are", function()
	local history = leaves({ 100, 100, 100 })
	t.equal(tree.is_ready(history, tree.leaf(2)), true)
	t.equal(tree.is_ready(history, tree.leaf(3)), false)
	t.equal(tree.is_ready(history, tree.node(1, 0)), true)
	tree.set(history, tree.node(1, 0), "a")
	history.nodes[tree.key(tree.leaf(1))] = nil
	local restored = tree.restore(history.entries, history.nodes, {})
	t.equal(tree.is_ready(restored, tree.node(1, 0)), false)
	t.equal(tree.is_ready(restored, tree.node(2, 0)), false)
	-- Its children not built, a parent cannot be opened either.
	t.equal({ tree.zoom(restored, 0, 2) }, { nil, "No line 0+2." })
end)

t.case("a view at its budget wants nothing; a byte over, one merge", function()
	local history = leaves({ 300, 300, 300, 300, 300, 300, 300, 300 })
	t.equal(tree.wanted(history, 2400), {})
	t.equal(tree.wanted(history, 2399), { tree.node(1, 0) })
end)

t.case("words and trim know Unicode's white space", function()
	t.equal(tree.words("\u{3000}a\u{a0}\u{2028} b\t\n"), "a b")
	t.equal(tree.trim("\u{85} a b \u{2009}"), "a b")
	t.equal(tree.trim("\u{2009}\u{a0}"), "")
	t.equal(tree.trim("é"), "é")
end)
