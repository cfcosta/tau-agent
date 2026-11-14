-- The history as OptChat keeps it (docs/reference/tree-compaction.md):
-- every message folded so far as entries, word for word; a binary tree
-- of one-line summaries over them; and the view, the list of nodes that
-- tiles the whole history within a byte budget.
--
-- Node `{ level, index }` covers entries `[index·2^level,
-- (index+1)·2^level)`. A level-0 node summarizes one entry; a parent
-- merges its two children. Nodes are named `id+n` by the first entry
-- they cover and how many: the label the model reads in the view and
-- passes to `zoom`. Entry ids count from 0.
--
-- A history is `{ entries, nodes, view }`: `entries` each `{ kind, text }`,
-- `nodes` each built node's line by its key (`level:index`), and `view`
-- its nodes in order.
local tree = {}

-- The target size of one summary line, in UTF-8 bytes. A target, not a
-- bound: a line the model could not bring under it keeps its shortest
-- form, and the view measures real sizes.
tree.NODE_BYTES = 512

-- The most characters an entry keeps of one message; a longer one keeps
-- its head and its tail.
tree.ENTRY_CHARS = 30000

-- White space as Unicode has it, beyond ASCII's, as UTF-8.
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
function tree.words(text)
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
function tree.trim(text)
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

-- `text` cut to `ENTRY_CHARS` characters, its head and its tail kept
-- around a note of what was cut.
function tree.cap(text)
	local total = utf8.len(text)
	if total <= tree.ENTRY_CHARS then
		return text
	end
	local keep = tree.ENTRY_CHARS // 2
	local head = string.sub(text, 1, utf8.offset(text, keep + 1) - 1)
	local tail = string.sub(text, utf8.offset(text, total - keep + 1))
	return head .. "\n[… " .. (total - 2 * keep) .. " characters cut …]\n" .. tail
end

-- An entry as its own line: `kind: text`.
function tree.line(entry)
	return entry.kind .. ": " .. entry.text
end

-- A message's text with its images as `[image]`, each block on its own
-- line.
local function with_images(message)
	if message.parts == nil then
		return message.text
	end
	local parts = {}
	for _, part in ipairs(message.parts) do
		table.insert(parts, part.image and "[image]" or part.text)
	end
	return table.concat(parts, "\n")
end

-- A message, as `rewrite_context` reads it, as entries: a person's
-- message is one; an answer is its text, if any, then one per call (its
-- thinking left out); a result is one, named by its tool.
function tree.entries(message)
	local out = {}
	if message.role == "user" then
		local text = with_images(message)
		if tree.trim(text) ~= "" then
			table.insert(out, { kind = "user", text = tree.cap(text) })
		end
	elseif message.role == "assistant" then
		if tree.trim(message.text) ~= "" then
			table.insert(out, { kind = "talk", text = tree.cap(message.text) })
		end
		for _, call in ipairs(message.calls or {}) do
			table.insert(out, { kind = "tool", text = tree.cap(call.name .. " " .. call.json) })
		end
	else
		local failed = message.ok == false and " failed" or ""
		table.insert(out, { kind = "echo", text = tree.cap("[" .. message.name .. failed .. "] " .. with_images(message)) })
	end
	return out
end

function tree.node(level, index)
	return { level = level, index = index }
end

-- The level-0 node of entry `id`.
function tree.leaf(id)
	return tree.node(0, id)
end

function tree.key(node)
	return node.level .. ":" .. node.index
end

-- How many entries it covers.
function tree.count(node)
	return 2 ^ node.level
end

-- The first entry it covers.
function tree.first(node)
	return node.index * 2 ^ node.level
end

-- One past the last entry it covers.
function tree.ending(node)
	return (node.index + 1) * 2 ^ node.level
end

function tree.parent(node)
	return tree.node(node.level + 1, node.index // 2)
end

-- Its two children; nil for a level-0 node.
function tree.children(node)
	if node.level == 0 then
		return nil
	end
	return tree.node(node.level - 1, node.index * 2), tree.node(node.level - 1, node.index * 2 + 1)
end

-- The node `zoom(id, n)` names, when `n` is a power of two and `id` a
-- multiple of it.
function tree.named(id, n)
	if n < 1 or id < 0 or n ~= math.floor(n) or id ~= math.floor(id) then
		return nil
	end
	local level, left = 0, n
	while left % 2 == 0 do
		left //= 2
		level += 1
	end
	if left ~= 1 or id % n ~= 0 then
		return nil
	end
	return tree.node(level, id // n)
end

-- Its name in the view: `id+n`.
function tree.label(node)
	return tree.first(node) .. "+" .. tree.count(node)
end

-- A shallow copy of `list`: `table.clone` refuses an array's metatable.
function tree.copy(list)
	local out = {}
	for key, value in pairs(list) do
		out[key] = value
	end
	return out
end

local function same(a, b)
	return a.level == b.level and a.index == b.index
end

-- An empty history.
function tree.new()
	return { entries = {}, nodes = {}, view = {} }
end

-- A history from what was kept of one: its entries, its built nodes and
-- its view. Nodes past the entries are dropped, and a view that does not
-- tile the entries in order is rebuilt from their level-0 nodes.
function tree.restore(entries, nodes, view)
	local total = #entries
	local kept = {}
	for key, text in pairs(nodes) do
		local level, index = string.match(key, "^(%d+):(%d+)$")
		local node = tree.node(tonumber(level), tonumber(index))
		if tree.ending(node) <= total then
			kept[key] = text
		end
	end
	local history = { entries = entries, nodes = kept, view = {} }
	local at = 0
	local tiles = true
	for _, node in ipairs(view) do
		if tree.first(node) ~= at or kept[tree.key(node)] == nil then
			tiles = false
			break
		end
		at = tree.ending(node)
	end
	if tiles and at == total then
		history.view = view
	else
		for id = 0, total - 1 do
			table.insert(history.view, tree.leaf(id))
		end
	end
	return history
end

function tree.text(history, node)
	return history.nodes[tree.key(node)]
end

function tree.is_built(history, node)
	return history.nodes[tree.key(node)] ~= nil
end

-- Appends `entry` and its level-0 node to the view, unbuilt, and gives
-- its id.
function tree.push(history, entry)
	local id = #history.entries
	table.insert(history.entries, entry)
	table.insert(history.view, tree.leaf(id))
	return id
end

-- Sets a node's line.
function tree.set(history, node, text)
	assert(tree.ending(node) <= #history.entries, tree.label(node) .. " is past the history")
	history.nodes[tree.key(node)] = text
end

-- Whether a node's sources are there: its entry, or its two children
-- built.
function tree.is_ready(history, node)
	local a, b = tree.children(node)
	if a == nil then
		return node.index < #history.entries
	end
	return tree.is_built(history, a) and tree.is_built(history, b)
end

-- A node's line when it needs no model: a level-0 node whose entry fits
-- in `NODE_BYTES` is the entry, word for word; a parent whose children
-- fit together is the two of them, one per line.
function tree.free(history, node)
	local a, b = tree.children(node)
	local text
	if a == nil then
		local entry = history.entries[node.index + 1]
		if entry == nil then
			return nil
		end
		text = tree.line(entry)
	else
		local first, second = tree.text(history, a), tree.text(history, b)
		if first == nil or second == nil then
			return nil
		end
		text = first .. "\n" .. second
	end
	if #text <= tree.NODE_BYTES then
		return text
	end
	return nil
end

-- A node's line's bytes, or `unbuilt` when it is not built.
local function bytes_of(history, node, unbuilt)
	local text = tree.text(history, node)
	return text and #text or unbuilt
end

-- The view's size: the bytes of its lines' texts, an unbuilt line
-- counted at `unbuilt` bytes.
function tree.view_bytes(history, unbuilt)
	local size = 0
	for _, node in ipairs(history.view) do
		size += bytes_of(history, node, unbuilt)
	end
	return size
end

-- The place in `view` of the pair to merge next: two siblings side by
-- side whose parent `ready` accepts, the most due first. A pair at level
-- `l` from entry `start` is due `(total - start) / 2^(l+2)`: detail fades
-- with age while each level keeps about as many lines. Ties go to the
-- older pair.
local function most_due(view, total, ready)
	local best, top = nil, nil
	for at = 1, #view - 1 do
		local a, b = view[at], view[at + 1]
		if a.level == b.level and a.index % 2 == 0 and ready(tree.parent(a)) then
			local due = (total - tree.first(a)) / 2 ^ (a.level + 2)
			if top == nil or due > top then
				best, top = at, due
			end
		end
	end
	return best
end

-- Merges the view's lines, most due first, until it fits in `budget`
-- bytes, passing over merges whose line is not built yet (OptChat's
-- `fit`): `"fits"`, or `"waiting"` when over it with nothing to merge.
-- Never splits a line.
function tree.fit(history, budget)
	local size = tree.view_bytes(history, tree.NODE_BYTES)
	local total = #history.entries
	while size > budget do
		local at = most_due(history.view, total, function(parent)
			return tree.is_built(history, parent)
		end)
		if at == nil then
			return "waiting"
		end
		local a, b = history.view[at], history.view[at + 1]
		local parent = tree.parent(a)
		size = size + bytes_of(history, parent, 0) - bytes_of(history, a, tree.NODE_BYTES) - bytes_of(history, b, tree.NODE_BYTES)
		history.view[at] = parent
		table.remove(history.view, at + 1)
	end
	return "fits"
end

-- The parents `fit` would merge into, in order, if every line were
-- built, an unbuilt one measured as the most it can be: what to build so
-- the view fits in `budget`.
function tree.wanted(history, budget)
	local view = tree.copy(history.view)
	local sizes = {}
	local size = 0
	for _, node in ipairs(view) do
		local text = tree.text(history, node)
		local bytes = text and #text or tree.NODE_BYTES
		sizes[tree.key(node)] = bytes
		size += bytes
	end
	local total = #history.entries
	local wanted = {}
	while size > budget do
		local at = most_due(view, total, function()
			return true
		end)
		if at == nil then
			break
		end
		local a, b = view[at], view[at + 1]
		local parent = tree.parent(a)
		local text = tree.text(history, parent)
		local parent_size = text and #text
			or math.min(sizes[tree.key(a)] + 1 + sizes[tree.key(b)], tree.NODE_BYTES)
		size = size + parent_size - sizes[tree.key(a)] - sizes[tree.key(b)]
		sizes[tree.key(parent)] = parent_size
		if text == nil then
			table.insert(wanted, parent)
		end
		view[at] = parent
		table.remove(view, at + 1)
	end
	return wanted
end

-- A node's line on one line.
function tree.flat(history, node)
	local text = tree.text(history, node)
	if text == nil then
		return "(no summary yet: zoom to read it)"
	end
	return tree.words(text)
end

-- The view as the model reads it: `id+n|text` a line, oldest first,
-- newlines in a text shown as spaces.
function tree.render(history)
	local lines = {}
	for _, node in ipairs(history.view) do
		table.insert(lines, tree.label(node) .. "|" .. tree.flat(history, node))
	end
	return table.concat(lines, "\n")
end

-- The view's lines before entry `ending`, bare (no labels), up to the
-- first that is not built: what a compactor call reads as context. Ids
-- in a compactor's input get copied into its output (OptChat, section
-- 4).
function tree.context(history, ending)
	local lines = {}
	for _, node in ipairs(history.view) do
		if tree.ending(node) > ending or not tree.is_built(history, node) then
			break
		end
		table.insert(lines, tree.flat(history, node))
	end
	return table.concat(lines, "\n")
end

-- What `zoom(id, n)` answers: line `id+n` opened into the two lines
-- under it, or with `n = 1`, entry `id` whole; else nil and why.
function tree.zoom(history, id, n)
	local missing = "No line " .. id .. "+" .. n .. "."
	local node = tree.named(id, n)
	if node == nil or tree.ending(node) > #history.entries then
		return nil, missing
	end
	local a, b = tree.children(node)
	if a == nil then
		return id .. "+1|" .. tree.line(history.entries[id + 1])
	end
	if not tree.is_built(history, a) or not tree.is_built(history, b) then
		return nil, missing
	end
	return tree.label(a) .. "|" .. tree.flat(history, a) .. "\n" .. tree.label(b) .. "|" .. tree.flat(history, b)
end

-- Whether two nodes are the same.
tree.same = same

return tree
