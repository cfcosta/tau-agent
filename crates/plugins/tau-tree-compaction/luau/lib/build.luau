-- Building the tree's lines with a model: a compactor after OptChat's
-- (docs/reference/tree-compaction.md, "Building lines"), one request per
-- line, many at once.
--
-- A line is built from the history's view as context, bare, and the
-- step: one entry, whole, to compress, or two lines to merge. The model
-- cannot count bytes, so it gets a line of exactly `NODE_BYTES` for
-- scale, and a reply over the limit is sent back, cut where the limit
-- falls, up to `TRIES` times; the shortest try is kept.
local tree = require("tree")

local build = {}

-- Tries per line to get it under `NODE_BYTES`.
build.TRIES = 5

-- The most characters of the entries just before a level-0 line's own
-- that its request shows, newest kept.
build.RECENT_CHARS = 6000

-- The most characters one recent entry takes of `RECENT_CHARS`.
local RECENT_ENTRY_CHARS = 1200

-- A realistic line of exactly `NODE_BYTES` bytes, dense and tagged as a
-- real one is, for scale.
build.SCALE = [==[
user: wants the parser to accept trailing commas in arrays and objects, keep errors at the comma's position, and not touch the lexer; said "the AST shape stays as is"; tool: read src/parse.rs: parse_list loops on Token::Comma and bails on RBracket; echo: cargo test failed 2 of 41, trailing_comma_array and nested_object, both expected Ok got Err(Unexpected(RBracket)) at 1:9; talk: fixed parse_list to stop on a closer after a comma; echo: tests pass 41 of 41; user: approved the change, asked for a commit next]==]

-- The compactor's instructions: how a line is built and what it keeps,
-- written for tau after OptChat's design.
build.COMPACT_PROMPT = [==[
You keep the memory of tau, an AI coding agent working for one person. A run of tau's can grow past what its context window holds; the older part of the run then reaches tau only as short summary lines, and you write them. Messages come in four kinds, each with a tag: user is what the person typed, talk is what tau answered, tool is a tool call tau made, and echo is what a tool returned.

The lines form a binary tree. Every message is first boiled down into a line of its own (a message already short enough stands as it is). Neighboring lines are then joined two at a time, so a line one level up covers two messages, the next level four, and so on. Each request gives you a single step of this: boil one message down into a line, or join two neighboring lines into one.

tau reads a long run as fine lines near its end and coarser ones further back. The line you write replaces the messages under it from then on, and will itself be joined with its neighbor later. tau can expand a line back into the two it came from, all the way down to the messages, but it does so only when the line hints that what it is looking for is in there. Something your line leaves out is gone for tau, and for every line built on top of yours.

<chat> shows how tau sees the run before your part, and <recent>, when present, is a shortened copy of the messages just before yours. Read them for the situation, for what "it" or "that file" points at, and for detail your input is missing.

The aim is that tau can carry on as if every message of your part were still in front of it. Bytes are scarce, so spend them where they are worth most:

1. First, the person. Their requests, choices, corrections and stated preferences, and more than anything the reasons they gave, should come through in their own wording as far as the bytes allow, and should be the last thing to go as lines are joined. Write down the content of what they said, never a bare "the user commented". Words are the person's only if the person wrote them.

2. Second, outcomes that last, whoever caused them: what changed in files or systems, what was agreed or promised, and anything that broke, with the cause.

3. Third, what was learned, what is still unresolved, and tau's own replies, which get much less room than the person's words.

4. Last, the working steps: tool calls and what they printed. They make up most of the log and little of its value. Do not paste them; sum each up in a handful of words: the action, whether it succeeded (with the error if it did not), what the thing it touched is and holds, and how that bears on the task at hand, even when it does not. Later this tells tau what has been tried and where things live, even for work nobody has thought of yet.

Shrink before you delete. A thing named in no line cannot be reached by expanding anything, while one or two words keep a path to it. When bytes run short, let the weighty items take most of them and give the small ones just a name; cut something outright only when tau is unlikely ever to want it and its bytes buy much more elsewhere.

You cannot know what will sit next to your line, so it has to read clearly alone. Begin each item with its tag, as in "user: ...; echo: ...". You are a recorder: do not reply to the messages, carry out what they ask, or add anything of your own, and report progress exactly as far as it went, calling nothing finished or working that the messages do not show to be so. Reply with the line and nothing else. Bytes are counted in UTF-8, so a character outside ASCII takes 2 to 4.]==]

-- `text`'s first `bytes` characters at most.
local function first_chars(text, chars)
	local cut = utf8.offset(text, chars + 1)
	if cut == nil then
		return text
	end
	return string.sub(text, 1, cut - 1)
end

-- The entries before `id` after the view's built lines (those
-- `tree.context` shows), newest kept, within `RECENT_CHARS`.
local function recent(history, id)
	local summarized = 0
	for _, node in ipairs(history.view) do
		if tree.ending(node) > id or not tree.is_built(history, node) then
			break
		end
		summarized = tree.ending(node)
	end
	local lines = {}
	local left = build.RECENT_CHARS
	for at = id, summarized + 1, -1 do
		local line = first_chars(tree.words(tree.line(history.entries[at])), RECENT_ENTRY_CHARS)
		local chars = utf8.len(line)
		if chars > left then
			break
		end
		left -= chars
		table.insert(lines, 1, line)
	end
	return table.concat(lines, "\n")
end

-- What a line's request reads first: the view up to the line, bare; and
-- for a level-0 line, the entries just before its own, cut short.
function build.context_block(history, node)
	local text = "<chat>\n" .. tree.context(history, tree.first(node)) .. "\n</chat>"
	if node.level == 0 then
		local before = recent(history, node.index)
		if before ~= "" then
			text ..= "\n<recent>\n" .. before .. "\n</recent>"
		end
	end
	return text
end

-- The step a line's request asks for: one entry, whole, to compress, or
-- two lines to merge, each written out again under the instruction.
function build.step(history, node)
	local a, b = tree.children(node)
	local what
	if a == nil then
		what = "Compress this message into one line, in at most "
			.. tree.NODE_BYTES
			.. " bytes:\n"
			.. tree.line(history.entries[node.index + 1])
	else
		what = "Merge these two lines into one, in at most "
			.. tree.NODE_BYTES
			.. " bytes:\n"
			.. tree.flat(history, a)
			.. "\n"
			.. tree.flat(history, b)
	end
	return "For scale, this line is exactly " .. tree.NODE_BYTES .. " bytes:\n" .. build.SCALE .. "\n\n" .. what
end

-- `text`'s first `bytes` bytes, a character the cut would split left
-- out.
function build.cut_bytes(text, bytes)
	local ending = math.min(bytes, #text)
	while ending > 0 and ending < #text and bit32.band(string.byte(text, ending + 1), 0xC0) == 0x80 do
		ending -= 1
	end
	return string.sub(text, 1, ending)
end

-- What a line over the limit is sent back with: its size, and the line
-- cut where the limit falls.
function build.too_long(line)
	return "That line is "
		.. #line
		.. " bytes; the limit is "
		.. tree.NODE_BYTES
		.. ". It must end where it is cut here:\n"
		.. build.cut_bytes(line, tree.NODE_BYTES)
		.. "| ← LIMIT"
end

-- One line, through as many tries as it takes to fit, or the shortest
-- of `TRIES`: its text, or nil and why. `ask(request)` asks the model
-- once, as `infer` does.
function build.line(history, node, ask, effort)
	local label = tree.label(node)
	local first = { build.context_block(history, node), build.step(history, node) }
	local messages = {}
	local task = first
	local tries = {}
	while true do
		local answer = ask({
			instructions = build.COMPACT_PROMPT,
			messages = array(tree.copy(messages)),
			task = task,
			effort = effort,
		})
		if not answer.ok then
			return nil, "the compactor failed on " .. label .. ": " .. tostring(answer.error)
		end
		local line = tree.trim(answer.value)
		if line == "" then
			return nil, "the compactor gave " .. label .. " an empty line"
		end
		table.insert(tries, line)
		if #line <= tree.NODE_BYTES or #tries == build.TRIES then
			break
		end
		table.insert(messages, { role = "user", text = task })
		table.insert(messages, { role = "assistant", text = answer.value })
		task = build.too_long(line)
	end
	local shortest = tries[1]
	for _, line in ipairs(tries) do
		if #line < #shortest then
			shortest = line
		end
	end
	return shortest
end

-- Builds `nodes`, whose sources are all there: free ones at once, the
-- rest `jobs` at a time, each against the history as it stood before any
-- of them. Adds to `grown`; the first failure, if any.
local function build_all(history, nodes, ask, options, grown)
	local asked = {}
	for _, node in ipairs(nodes) do
		local text = tree.free(history, node)
		if text ~= nil then
			tree.set(history, node, text)
			table.insert(grown.built, node)
		else
			table.insert(asked, node)
		end
	end
	-- Each request reads the history as it is now.
	local snapshot = { entries = history.entries, nodes = tree.copy(history.nodes), view = tree.copy(history.view) }
	local results = {}
	local jobs = math.max(options.jobs or 1, 1)
	for from = 1, #asked, jobs do
		local batch = {}
		for at = from, math.min(from + jobs - 1, #asked) do
			local node = asked[at]
			table.insert(batch, function()
				local text, why = build.line(snapshot, node, ask, options.effort)
				return { node = node, text = text, why = why }
			end)
		end
		for _, result in ipairs({ parallel(table.unpack(batch)) }) do
			table.insert(results, result)
		end
	end
	local failed
	for _, result in ipairs(results) do
		if result.text ~= nil then
			tree.set(history, result.node, result.text)
			table.insert(grown.built, result.node)
			grown.asked += 1
		elseif failed == nil then
			failed = result.why
		end
	end
	return failed
end

-- Builds every unbuilt line the view holds, then merges until the view
-- fits in `budget` bytes, building the merged lines a level at a time,
-- `options.jobs` requests at once, asking at `options.effort`. Lines that
-- need no model are set without a request. What it built: how many lines
-- took a request, and every line built; or nil and why.
function build.grow(history, budget, ask, options)
	local grown = { asked = 0, built = {} }
	local leaves = {}
	for _, node in ipairs(history.view) do
		if not tree.is_built(history, node) then
			table.insert(leaves, node)
		end
	end
	local failed = build_all(history, leaves, ask, options, grown)
	if failed ~= nil then
		return nil, failed
	end
	while tree.fit(history, budget) ~= "fits" do
		local ready = {}
		for _, node in ipairs(tree.wanted(history, budget)) do
			if tree.is_ready(history, node) then
				table.insert(ready, node)
			end
		end
		if #ready == 0 then
			return nil,
				"the view is "
					.. tree.view_bytes(history, tree.NODE_BYTES)
					.. " bytes, over its "
					.. budget
					.. ", and nothing is left to merge"
		end
		failed = build_all(history, ready, ask, options, grown)
		if failed ~= nil then
			return nil, failed
		end
	end
	return grown
end

return build
