-- The transcript as the history stage shows Jev: tool calls with their
-- inputs and the size of their results, never the results themselves.
--
-- Ported from `tamaratran/fast-jev-compaction` by way of
-- `joelhooks/pi-fast-jev-compaction` (`src/core/state.ts`; see
-- THIRD_PARTY_NOTICES.md). Lengths count characters. What Jev reads is
-- written here as JSON text, its keys in order, as the Rust plugin's
-- types wrote it: a Luau table keeps no order.
local estimate = require("estimate")

local state = {}

-- What the state tells Jev about itself.
state.CONTEXT = [==[
A coding assistant conversation is being compacted to free context. `history` is the conversation so far, oldest first, verbatim except that tool outputs are replaced by a short `result` note. A long conversation is split into ordered segments that are asked about separately; then `history` is one segment, `calls` lists the tool calls asked about, and a keep in any segment keeps the call. Oversized fields continue across entries labeled `part`, with their field name and character offset. Each question asks whether one tool call, or the full output of that call, still needs to stay in the history verbatim. Whatever is not kept is deleted from model context: a cut output keeps its head and the path of a file holding it whole, and a dropped call goes with its output, though the assistant can re-run a tool or re-read a file.]==]

-- White space as Unicode has it, by code point.
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

local ESCAPES = { ['"'] = '\\"', ["\\"] = "\\\\", ["\b"] = "\\b", ["\f"] = "\\f", ["\n"] = "\\n", ["\r"] = "\\r", ["\t"] = "\\t" }

-- `text` as a JSON string, escaped as serde_json escapes it, at any
-- length: `json.encode` takes a megabyte at most.
function state.string(text)
	local escaped = string.gsub(text, '[%c"\\]', function(c)
		if c == "\127" then
			return c
		end
		return ESCAPES[c] or string.format("\\u%04x", string.byte(c))
	end)
	return '"' .. escaped .. '"'
end

-- Whether `text` is empty or white space alone.
function state.blank(text)
	for _, c in utf8.codes(text) do
		if not SPACE[c] then
			return false
		end
	end
	return true
end

-- A JSON object's text from `fields`, each `{ key, text }` with `text`
-- its value's JSON, in order.
function state.object(fields)
	local parts = {}
	for _, field in ipairs(fields) do
		table.insert(parts, json.encode(field[1]) .. ":" .. field[2])
	end
	return "{" .. table.concat(parts, ",") .. "}"
end

-- A JSON array's text from its items' texts.
function state.list(texts)
	return "[" .. table.concat(texts, ",") .. "]"
end

-- An answer's text as the stages read it: its text and its thinking, in
-- order, by new lines; the thinking marked so.
local function said(message)
	if message.parts == nil then
		return message.text
	end
	local parts = {}
	for _, part in ipairs(message.parts) do
		if part.thinking ~= nil then
			table.insert(parts, "[thinking]\n" .. part.thinking)
		elseif part.text ~= nil then
			table.insert(parts, part.text)
		end
	end
	return table.concat(parts, "\n")
end

-- A transcript, as `rewrite_context` reads it, reduced to what the
-- decisions need: each message's role (a result counts as the person's),
-- its text, the calls it makes, with their inputs as JSON, and the
-- results it holds.
function state.entries(messages)
	local entries = {}
	for _, message in ipairs(messages) do
		local entry = { role = "user", text = "", uses = {}, outcomes = {} }
		if message.role == "user" then
			entry.text = message.text
		elseif message.role == "assistant" then
			entry.role = "assistant"
			entry.text = said(message)
			for _, call in ipairs(message.calls or {}) do
				table.insert(entry.uses, { call_id = call.id, tool = call.name, input = call.json, args = call.args })
			end
		else
			table.insert(entry.outcomes, { call_id = message.call, text = message.text, is_error = message.ok == false })
		end
		table.insert(entries, entry)
	end
	return entries
end

-- Whether the message at `index` (from 0) is pinned: the first, or one
-- of the last `preserve`.
function state.is_pinned(index, total, preserve)
	return index == 0 or index + preserve >= total
end

-- Every tool call that has a result, in order, as `{ id, call_id, tool,
-- input, call_index, result_index, result_chars, is_error, pinned }`,
-- places from 0; pinned when its call or its result is. `id` is `t1`,
-- `t2`…: short, and free of the provider's id.
function state.calls(entries, preserve)
	local results = {}
	for index, entry in ipairs(entries) do
		for _, outcome in ipairs(entry.outcomes) do
			results[outcome.call_id] = { index = index - 1, outcome = outcome }
		end
	end
	local calls = {}
	for index, entry in ipairs(entries) do
		for _, use in ipairs(entry.uses) do
			local found = results[use.call_id]
			if found ~= nil then
				table.insert(calls, {
					id = "t" .. (#calls + 1),
					call_id = use.call_id,
					tool = use.tool,
					input = use.input,
					call_index = index - 1,
					result_index = found.index,
					result_chars = utf8.len(found.outcome.text),
					is_error = found.outcome.is_error,
					pinned = state.is_pinned(index - 1, #entries, preserve)
						or state.is_pinned(found.index, #entries, preserve),
				})
			end
		end
	end
	return calls
end

-- A call's note in place of its result: its status and size.
function state.result_note(call)
	return (call.is_error and "error" or "ok") .. ", " .. call.result_chars .. " chars (omitted)"
end

-- A call as the state shows it: its id, tool, whole input, and a note of
-- its result's status and size.
function state.call_record(call)
	return { id = call.id, tool = call.tool, input = call.input, result = state.result_note(call) }
end

-- A call record's JSON text.
function state.call_json(record)
	local fields = {
		{ "id", state.string(record.id) },
		{ "tool", state.string(record.tool) },
		{ "input", state.string(record.input) },
	}
	if record.result ~= nil then
		table.insert(fields, { "result", state.string(record.result) })
	end
	return state.object(fields)
end

-- A result record's JSON text: a result, verbatim.
function state.result_json(record)
	local fields = { { "id", state.string(record.id) } }
	if record.is_error then
		table.insert(fields, { "is_error", "true" })
	end
	table.insert(fields, { "result", state.string(record.result) })
	return state.object(fields)
end

-- A history record's JSON text: `{ i, role, text, tool_calls?,
-- tool_results?, part? }`.
function state.record_json(record)
	local fields = {
		{ "i", tostring(record.i) },
		{ "role", state.string(record.role) },
		{ "text", state.string(record.text) },
	}
	if #record.tool_calls > 0 then
		local calls = {}
		for _, call in ipairs(record.tool_calls) do
			table.insert(calls, state.call_json(call))
		end
		table.insert(fields, { "tool_calls", state.list(calls) })
	end
	if #record.tool_results > 0 then
		local results = {}
		for _, result in ipairs(record.tool_results) do
			table.insert(results, state.result_json(result))
		end
		table.insert(fields, { "tool_results", state.list(results) })
	end
	if record.part ~= nil then
		table.insert(fields, {
			"part",
			state.object({
				{ "field", state.string(record.part.field) },
				{ "offset", tostring(record.part.offset) },
				{ "total_chars", tostring(record.part.total_chars) },
			}),
		})
	end
	return state.object(fields)
end

-- Records' JSON text, as a list.
function state.records_json(records)
	local texts = {}
	for _, record in ipairs(records) do
		table.insert(texts, state.record_json(record))
	end
	return state.list(texts)
end

-- The history of the state: every message with text or tool calls, in
-- order, whole. Tool calls come with their input and a note of their
-- result; the results themselves never do.
function state.history(entries, calls)
	local by_message = {}
	for _, call in ipairs(calls) do
		by_message[call.call_index] = by_message[call.call_index] or {}
		table.insert(by_message[call.call_index], state.call_record(call))
	end
	local records = {}
	for index, entry in ipairs(entries) do
		local tool_calls = by_message[index - 1] or {}
		if not state.blank(entry.text) or #tool_calls > 0 then
			table.insert(records, {
				i = index - 1,
				role = entry.role,
				text = entry.text,
				tool_calls = tool_calls,
				tool_results = {},
			})
		end
	end
	return records
end

-- The person's last three prompts, each cut to 500 characters: the goal
-- when none is given.
function state.goal_from(entries)
	local prompts = {}
	for _, entry in ipairs(entries) do
		if entry.role == "user" and not state.blank(entry.text) and #entry.outcomes == 0 then
			table.insert(prompts, estimate.truncate(entry.text, 500))
		end
	end
	return table.concat(prompts, "\n", math.max(#prompts - 2, 1), #prompts)
end

-- `goal` when there is one, else `goal_from` `entries`.
function state.goal_or_prompts(goal, entries)
	if type(goal) == "string" and goal ~= "" then
		return goal
	end
	return state.goal_from(entries)
end

-- The state sent to Jev, as JSON text: the whole history, or one
-- segment of it with the calls asked about beside it, as texts.
function state.state_json(goal, history_json, calls_json)
	local fields = {
		{ "context", state.string(state.CONTEXT) },
		{ "goal", state.string(goal) },
		{ "history", history_json },
	}
	if calls_json ~= nil then
		table.insert(fields, { "calls", calls_json })
	end
	return state.object(fields)
end

return state
