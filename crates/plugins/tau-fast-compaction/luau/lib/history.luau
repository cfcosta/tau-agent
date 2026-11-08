-- The conversation as Jev sees it, whole: split in order into segments
-- that each fit a budget, never truncated and never left out.
--
-- Ported from `tamaratran/jev-pruner` (`src/history.ts`,
-- `splitHistory`; see THIRD_PARTY_NOTICES.md). A message too large for
-- a segment of its own becomes continuations, one per field it holds,
-- each labeled with the field's name and the character offset it starts
-- at. Offsets count characters, so a continuation never splits one.
local estimate = require("estimate")
local state = require("state")

local history = {}

-- The characters a continuation is first tried at, before doubling.
local FIRST_PROBE = 4096

-- Tenths of a token for the two quotes of an empty JSON string.
local QUOTES_TENTHS = 18

-- Tenths of a token for a comma between entries.
history.COMMA_TENTHS = 9

local FIELD_NAMES = {
	["text"] = "text",
	["tool_calls.input"] = "tool call's input",
	["tool_calls.result"] = "tool call's result note",
	["tool_results.result"] = "tool result",
}

local function tenths_of(record)
	return estimate.json_tenths(state.record_json(record))
end
history.tenths_of = tenths_of

-- `record`, holding only `piece` of one field, labeled as such.
-- `index` picks the call or result for fields of those.
local function fragment(record, field, index, piece, offset, total)
	local out = {
		i = record.i,
		role = record.role,
		text = "",
		tool_calls = {},
		tool_results = {},
		part = { field = field, offset = offset, total_chars = total },
	}
	if field == "text" then
		out.text = piece
	elseif field == "tool_calls.input" then
		local call = record.tool_calls[index]
		out.tool_calls = { { id = call.id, tool = call.tool, input = piece } }
	elseif field == "tool_calls.result" then
		local call = record.tool_calls[index]
		out.tool_calls = { { id = call.id, tool = call.tool, input = "", result = piece } }
	else
		local result = record.tool_results[index]
		out.tool_results = { { id = result.id, is_error = result.is_error, result = piece } }
	end
	return out
end

-- Every field `record` splits along, in order: its text when it has one,
-- then each call's input and result note, then each result.
local function fields(record)
	local out = {}
	if record.text ~= "" then
		table.insert(out, { "text", 0, record.text })
	end
	for index, call in ipairs(record.tool_calls) do
		table.insert(out, { "tool_calls.input", index, call.input })
		if call.result ~= nil then
			table.insert(out, { "tool_calls.result", index, call.result })
		end
	end
	for index, result in ipairs(record.tool_results) do
		table.insert(out, { "tool_results.result", index, result.result })
	end
	return out
end

-- `record` as it goes into segments: whole when it fits `max_tenths`
-- alone, else as continuations that each do, each with its tenths. A
-- continuation takes as many characters as fit, back to the last line
-- break when that keeps at least half of them. Nil and why when not
-- even one character of a field fits.
function history.split_record(record, max_tenths)
	local whole = tenths_of(record)
	if whole <= max_tenths then
		return { { record, whole } }
	end
	local fragments = {}
	for _, spec in ipairs(fields(record)) do
		local field, index, text = spec[1], spec[2], spec[3]
		local total = utf8.len(text)
		local offset, start = 0, 1
		-- `length` characters from the continuation's start.
		local function piece(length)
			local ending = utf8.offset(text, length + 1, start) or #text + 1
			return string.sub(text, start, ending - 1)
		end
		while true do
			local base = tenths_of(fragment(record, field, index, "", offset, total))
			local function fits(length)
				return base + estimate.json_tenths(state.string(piece(length))) - QUOTES_TENTHS <= max_tenths
			end
			local remaining = total - offset
			if not fits(0) or (remaining > 0 and not fits(1)) then
				return nil,
					"a "
						.. FIELD_NAMES[field]
						.. " of message "
						.. record.i
						.. " cannot fit in "
						.. max_tenths // 10
						.. " tokens"
			end
			-- Doubling from a first probe, then halving between the last
			-- length that fit and the first that did not.
			local low, high = 0, remaining
			local probe = math.min(FIRST_PROBE, remaining)
			while probe > low do
				if fits(probe) then
					low = probe
					probe = math.min(probe * 2, remaining)
				else
					high = probe - 1
					break
				end
			end
			while low < high do
				local mid = (low + high + 1) // 2
				if fits(mid) then
					low = mid
				else
					high = mid - 1
				end
			end
			if offset + low < total then
				local taken = piece(low)
				local last = nil
				local at = 1
				while true do
					local found = string.find(taken, "\n", at, true)
					if found == nil then
						break
					end
					last = found
					at = found + 1
				end
				if last ~= nil then
					local newline = utf8.len(taken, 1, last) - 1
					if 2 * newline >= low then
						low = newline + 1
					end
				end
			end
			local taken = piece(low)
			local made = fragment(record, field, index, taken, offset, total)
			table.insert(fragments, { made, tenths_of(made) })
			offset += low
			start += #taken
			if offset >= total then
				break
			end
		end
	end
	return fragments
end

-- `records` split, in order, into segments whose JSON estimate each fits
-- `max_tokens`, splitting a record that cannot fit alone into
-- continuations. Every record, or all its continuations, lands in
-- exactly one segment. No records make one empty segment. Nil and why
-- when a field cannot fit.
function history.split(records, max_tokens)
	local max_tenths = max_tokens * 10
	-- An array's brackets; each entry after the first adds a comma.
	local empty = estimate.json_tenths("[]")
	local segments = {}
	local current = {}
	local tenths = empty
	for _, record in ipairs(records) do
		local pieces, why = history.split_record(record, math.max(max_tenths - empty, 0))
		if pieces == nil then
			return nil, why
		end
		for _, made in ipairs(pieces) do
			local cost = made[2]
			local comma = #current == 0 and 0 or history.COMMA_TENTHS
			if #current > 0 and tenths + comma + cost > max_tenths then
				table.insert(segments, current)
				current = {}
				tenths = empty
			end
			comma = #current == 0 and 0 or history.COMMA_TENTHS
			tenths += comma + cost
			table.insert(current, made[1])
		end
	end
	if #current > 0 or #segments == 0 then
		table.insert(segments, current)
	end
	return segments
end

return history
