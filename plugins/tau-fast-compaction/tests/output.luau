-- Output pruning's pure parts, over drawn outputs and histories: the
-- gate, the lines and chunks, the rendering, the decisions, the
-- requests, the history's segments and the asking.
local tau = require("tau")
local t = tau.test
local g = t.gen
local estimate = require("estimate")
local state = require("state")
local history = require("history")
local plan = require("plan")
local decide = require("decide")
local output = require("output")

local function starts_with(text, part)
	return string.sub(text, 1, #part) == part
end

local function ends_with(text, part)
	return string.sub(text, -#part) == part
end

-- `chars` characters of `pieces` repeated, from piece `from`, cut with
-- `utf8.offset`.
local function cycle(pieces, chars, from)
	if chars == 0 then
		return ""
	end
	local rotated = {}
	for at = 1, #pieces do
		rotated[at] = pieces[(at + (from or 0) - 1) % #pieces + 1]
	end
	local text = string.rep(table.concat(rotated), chars // #pieces + 1)
	return string.sub(text, 1, utf8.offset(text, chars + 1) - 1)
end

-- Text of a drawn length up to `max` characters, made of a drawn
-- pattern of `alphabet`'s characters.
local function sized(alphabet, max)
	return g.composite(function(draw)
		local chars = draw(g.integers({ min = 0, max = max }))
		local pattern = draw(g.lists(g.sampled_from(alphabet), { min = 1, max = 8 }))
		return cycle(pattern, chars)
	end)
end

-- What an output line is made of: never `[` or a new line, so a line
-- never reads as a marker.
local LINE = { "a", "b", "c", " ", "x", "y", "z", "0", "1", "8", "9", ".", ":", "-", "_", "/", "é", "日", "🦀" }

-- An output: up to 300 lines, most short, some over `MAX_LINE_CHARS`
-- characters.
local output_text = g.composite(function(draw)
	local pattern = draw(g.lists(g.sampled_from(LINE), { min = 1, max = 12 }))
	local count = draw(g.integers({ min = 1, max = 300 }))
	local lines = {}
	for at = 1, count do
		if draw(g.integers({ min = 0, max = 40 })) == 0 then
			local chars = draw(g.integers({ min = output.MAX_LINE_CHARS + 1, max = 3 * output.MAX_LINE_CHARS + 7 }))
			table.insert(lines, cycle({ "a", "b", " ", "1" }, chars))
		else
			table.insert(lines, cycle(pattern, draw(g.integers({ min = 0, max = 60 })), at))
		end
	end
	return table.concat(lines, "\n")
end)

-- The gate lets through only a successful `exec_command` or
-- `write_stdin` result of text, without a NUL byte, not truncated,
-- estimated over the threshold (not at it), and then takes the result's
-- text as the whole output. Everything else goes untouched.
t.property("the gate passes only large successful text", function(draw)
	local tool = draw(g.sampled_from({ "exec_command", "write_stdin", "read" }))
	local ok = draw(g.booleans())
	local text = draw(output_text)
	if draw(g.booleans()) then
		text ..= "\0"
	end
	local tokens = estimate.tokens(text)
	local min = draw(g.one_of({
		g.sampled_from({ math.max(tokens - 1, 0), tokens, tokens + 1 }),
		g.integers({ min = 0, max = 3000 }),
	}))
	local images = draw(g.integers({ min = 0, max = 5 })) == 0 and 1 or nil
	local truncated = draw(g.integers({ min = 0, max = 5 })) == 0
	local expected = tool ~= "read"
		and ok
		and images == nil
		and not truncated
		and string.find(text, "\0", 1, true) == nil
		and tokens > min
	t.note(expected and "passes" or "untouched")
	local gated = output.gate({ name = tool, ok = ok, text = text, images = images, truncated = truncated }, min)
	t.equal(gated ~= nil, expected)
	if gated ~= nil then
		t.equal(gated, text)
	end
end, { cases = 200 })

-- `text` split at each new line.
local function split_lines(text)
	local out = {}
	local at = 1
	while true do
		local found = string.find(text, "\n", at, true)
		table.insert(out, string.sub(text, at, (found or #text + 1) - 1))
		if found == nil then
			return out
		end
		at = found + 1
	end
end

-- The lines of an output make it up exactly, none over
-- `MAX_LINE_CHARS`; its chunks cover them in order, at most
-- `MAX_CHUNKS`. Rendering any choice of chunks keeps every kept line
-- verbatim and in order, a long line's pieces together when kept
-- together, and puts one marker for each run of dropped lines, counting
-- exactly them; the header comes first, and the footer names the
-- archive.
t.property("rendering keeps lines verbatim and counts what it omits", function(draw)
	local text = draw(output_text)
	local lines = output.lines(text)
	local rebuilt = {}
	for index, line in ipairs(lines) do
		t.equal(utf8.len(line.text) <= output.MAX_LINE_CHARS, true)
		table.insert(rebuilt, line.text)
		if line.ends and index < #lines then
			table.insert(rebuilt, "\n")
		end
	end
	t.equal(table.concat(rebuilt), text)

	local chunk_lines = draw(g.integers({ min = 0, max = 30 }))
	local chunks = output.chunks(#lines, chunk_lines)
	t.equal(#chunks >= 1 and #chunks <= output.MAX_CHUNKS, true)
	t.equal(chunks[1][1], 1)
	t.equal(chunks[#chunks][2], #lines)
	for at = 2, #chunks do
		t.equal(chunks[at][1], chunks[at - 1][2] + 1)
	end

	local keep = {}
	for index in ipairs(chunks) do
		keep[index] = draw(g.booleans())
	end
	local archive = "/archive/of/it.txt"
	local rendered = output.render(lines, chunks, keep, archive)
	local header = output.HEADER .. "\n"
	local footer = "\n\n[full output: " .. archive .. " (read or grep it if needed)]"
	t.equal(starts_with(rendered, header), true)
	t.equal(ends_with(rendered, footer), true)
	local body = string.sub(rendered, #header + 1, #rendered - #footer)

	local expected = {}
	local open = false
	for index, chunk in ipairs(chunks) do
		for at = chunk[1], chunk[2] do
			local line = lines[at]
			local last = expected[#expected]
			if keep[index] then
				if last ~= nil and last.line ~= nil and open then
					last.line ..= line.text
				else
					table.insert(expected, { line = line.text })
				end
				open = not line.ends
			else
				if last ~= nil and last.omitted ~= nil then
					last.omitted += 1
				else
					table.insert(expected, { omitted = 1 })
				end
				open = false
			end
		end
	end
	local said = {}
	for _, line in ipairs(split_lines(body)) do
		local count = string.match(line, "^%[(%d+) lines omitted%]$")
		if count ~= nil then
			table.insert(said, { omitted = tonumber(count) })
		else
			table.insert(said, { line = line })
		end
	end
	t.equal(said, expected)
end, { cases = 200 })

-- A chunk goes only when it is neither the first nor the last, every
-- segment answered about it, and every answer was under the threshold;
-- an unanswered segment (a failed request, or one past the allowance)
-- keeps it.
t.property("only a chunk every segment scored under the threshold goes", function(draw)
	local chunks = draw(g.integers({ min = 1, max = 40 }))
	local segments = draw(g.integers({ min = 1, max = 4 }))
	local threshold = draw(g.sampled_from({ 0, 0.05, 0.1, 0.5, 0.9 }))
	local tally = plan.tally(chunks, segments)
	local answers = {}
	for chunk = 1, chunks do
		answers[chunk] = {}
		for segment = 1, segments do
			-- `false` for a segment that gave no answer.
			local answer = draw(g.sampled_from({ false, 0, 0.05, 0.1, 0.2, 0.49, 0.5, 1 }))
			if answer ~= false then
				plan.record(tally, chunk, segment, { answer })
			end
			answers[chunk][segment] = answer
		end
	end
	local keep = output.keep(chunks, tally, threshold)
	for chunk = 1, chunks do
		local disposable = chunk ~= 1 and chunk ~= chunks
		for segment = 1, segments do
			local answer = answers[chunk][segment]
			disposable = disposable and answer ~= false and answer < threshold
		end
		t.equal(keep[chunk], not disposable, "chunk " .. chunk)
	end
end, { cases = 300 })

local TEXT = { "a", "b", " ", "c", "d", "\n", "0", "1", "8", "9", ".", ",", '"', "{", "é", "🦀" }

-- A record of a drawn history, message `i`.
local function record(i)
	return g.composite(function(draw)
		local big = draw(g.integers({ min = 0, max = 4 })) == 0 and 6000 or 200
		local made = {
			i = i,
			role = draw(g.booleans()) and "user" or "assistant",
			text = draw(sized(TEXT, big)),
			tool_calls = {},
			tool_results = {},
		}
		for n = 0, draw(g.integers({ min = 0, max = 2 })) - 1 do
			table.insert(made.tool_calls, {
				id = "t" .. i .. n,
				tool = "exec_command",
				input = draw(sized(TEXT, big)),
				result = draw(g.booleans()) and draw(sized(TEXT, 40)) or nil,
			})
		end
		for n = 0, draw(g.integers({ min = 0, max = 2 })) - 1 do
			table.insert(made.tool_results, {
				id = "t" .. i .. n,
				is_error = draw(g.booleans()),
				result = draw(sized(TEXT, big)),
			})
		end
		return made
	end)
end

-- A drawn history of up to 8 records.
local records = g.composite(function(draw)
	local out = {}
	for i = 0, draw(g.integers({ min = 0, max = 8 })) - 1 do
		table.insert(out, draw(record(i)))
	end
	return out
end)

-- The fields of a record, as continuations name them, with the call or
-- result they belong to and their text.
local function fields(made)
	local out = {}
	if made.text ~= "" then
		table.insert(out, { "text", "", made.text })
	end
	for _, call in ipairs(made.tool_calls) do
		table.insert(out, { "tool_calls.input", call.id, call.input })
		if call.result ~= nil then
			table.insert(out, { "tool_calls.result", call.id, call.result })
		end
	end
	for _, result in ipairs(made.tool_results) do
		table.insert(out, { "tool_results.result", result.id, result.result })
	end
	return out
end

-- What a continuation holds: its field, the call or result it belongs
-- to, and its piece of the field.
local function piece(fragment)
	local field = fragment.part.field
	if field == "text" then
		return field, "", fragment.text
	elseif field == "tool_calls.input" then
		return field, fragment.tool_calls[1].id, fragment.tool_calls[1].input
	elseif field == "tool_calls.result" then
		return field, fragment.tool_calls[1].id, fragment.tool_calls[1].result
	end
	return field, fragment.tool_results[1].id, fragment.tool_results[1].result
end

-- Splitting a history loses nothing and truncates nothing: every record
-- lands in exactly one place, in order, whole when it fits alone,
-- otherwise as continuations of each of its fields that reassemble to
-- the field, with offsets counting the characters before; and every
-- segment's estimate fits the budget.
t.property("the history splits into segments that fit", function(draw)
	local history_records = draw(records)
	local budget = draw(g.integers({ min = 150, max = 4000 }))
	local segments, why = history.split(history_records, budget)
	t.equal(why, nil)
	t.equal(#segments >= 1, true)
	local placed = {}
	for _, segment in ipairs(segments) do
		local tenths = estimate.json_tenths(state.records_json(segment))
		local tokens = estimate.div_ceil(tenths, 10)
		t.equal(tokens <= budget, true, tokens .. " > " .. budget)
		t.equal(#segment > 0 or #history_records == 0, true)
		for _, made in ipairs(segment) do
			table.insert(placed, made)
		end
	end
	if #segments > 1 then
		t.note("several segments")
	end
	local at = 1
	for _, made in ipairs(history_records) do
		local first = placed[at]
		t.equal(first ~= nil, true, "a record went missing")
		if first.part == nil then
			t.equal(first, made)
			at += 1
		else
			for _, spec in ipairs(fields(made)) do
				local field, id, text = spec[1], spec[2], spec[3]
				local total = utf8.len(text)
				local joined, chars = {}, 0
				while placed[at] ~= nil do
					local fragment = placed[at]
					t.equal(fragment.part ~= nil, true, "a whole record inside a split one")
					local piece_field, piece_id, part_text = piece(fragment)
					if fragment.i ~= made.i or piece_field ~= field or piece_id ~= id then
						break
					end
					t.equal(fragment.role, made.role)
					t.equal(fragment.part.offset, chars)
					t.equal(fragment.part.total_chars, total)
					table.insert(joined, part_text)
					chars += utf8.len(part_text)
					at += 1
					if chars == total then
						break
					end
				end
				t.equal(table.concat(joined), text, field .. " of message " .. made.i)
			end
		end
	end
	t.equal(placed[at], nil, "a fragment belongs to no record")
end, { cases = 200 })

-- Settings with drawn budgets, small enough to split.
local budgets = g.composite(function(draw)
	local max_state = draw(g.integers({ min = 1500, max = 8000 }))
	return {
		chunk_lines = draw(g.integers({ min = 1, max = 25 })),
		max_state_tokens = max_state,
		max_request_tokens = max_state + draw(g.integers({ min = 300, max = 3000 })),
		max_output_requests = draw(g.integers({ min = 1, max = 15 })),
	}
end)

-- The requests for an output: none with two chunks or fewer; never more
-- than the allowance; every state fits its budget, and so does every
-- request with its questions; the first and last chunks are never asked
-- about; a chunk asked about is in the state and is asked about once in
-- every segment; and each state holds one segment, the segments making
-- up the whole history in order.
t.property("requests fit and stay within the allowance", function(draw)
	local text = draw(output_text)
	local history_records = draw(records)
	local settings = draw(budgets)
	local lines = output.lines(text)
	local planned, why = output.plan(lines, history_records, "fix the build", "cargo build", settings)
	t.equal(why, nil)
	local chunks = output.chunks(#lines, settings.chunk_lines)
	if planned == nil then
		t.equal(#chunks <= 2, true)
		return
	end
	t.equal(planned.chunks, chunks)
	t.equal(#planned.requests <= settings.max_output_requests, true)
	if #planned.requests == 0 then
		t.note("nothing to ask within the allowance")
	end
	local asked, histories = {}, {}
	for _, request in ipairs(planned.requests) do
		local tokens = estimate.state_tokens(request.state)
		t.equal(tokens <= settings.max_state_tokens, true)
		local questions = decide.questions_json(request.questions)
		t.equal(
			tokens + estimate.state_tokens(questions) + plan.REQUEST_OVERHEAD_TOKENS <= settings.max_request_tokens,
			true
		)
		local decoded = json.decode(request.state)
		local in_state = {}
		for _, chunk in ipairs(decoded.chunks) do
			in_state[chunk.id] = true
		end
		local ids, asked_ids = {}, {}
		for _, chunk in ipairs(request.asked) do
			table.insert(ids, output.chunk_id(chunk))
		end
		for _, question in ipairs(request.questions) do
			table.insert(asked_ids, question.id)
		end
		table.sort(ids)
		table.sort(asked_ids)
		t.equal(asked_ids, ids)
		for _, chunk in ipairs(request.asked) do
			t.equal(chunk ~= 1 and chunk ~= #chunks, true)
			t.equal(in_state[output.chunk_id(chunk)], true)
			asked[chunk] = asked[chunk] or {}
			t.equal(asked[chunk][request.segment], nil, "asked twice in a segment")
			asked[chunk][request.segment] = true
		end
		histories[request.segment] = decoded.history
	end
	for chunk, segments in pairs(asked) do
		local count = 0
		for _ in pairs(segments) do
			count += 1
		end
		t.equal(count, planned.segments, "chunk " .. chunk)
	end
	local all, complete = {}, true
	for segment = 1, planned.segments do
		if histories[segment] == nil then
			complete = false
		else
			for _, made in ipairs(histories[segment]) do
				if all[#all] ~= made.i then
					table.insert(all, made.i)
				end
			end
		end
	end
	if complete then
		local whole = {}
		for _, made in ipairs(history_records) do
			table.insert(whole, made.i)
		end
		t.equal(all, whole)
	end
end, { cases = 200 })

-- 400 lines of a build, planned with budgets small enough for several
-- requests.
local function build_plan()
	local steps = {}
	for n = 0, 399 do
		table.insert(steps, "step " .. n .. " of the build")
	end
	local lines = output.lines(table.concat(steps, "\n"))
	local planned = output.plan(lines, {}, "task", "make", {
		chunk_lines = 20,
		max_state_tokens = 2000,
		max_request_tokens = 2600,
		max_output_requests = 50,
	})
	return planned
end

-- Jev's answers to every question of `req`: `noul(id)`'s probability.
local function answering(req, noul)
	local answers = {}
	for id in pairs(req.questions) do
		answers[id] = { probability = noul(id) }
	end
	return answers
end

-- Asking sends every planned request and charges each one answered;
-- with every answer given, a chunk goes exactly when `output.keep` says:
-- the second chunk, kept by its answer, stays with the first and last.
t.case("asking charges every answer and keeps what Jev keeps", function()
	local planned = build_plan()
	t.equal(#planned.requests > 1, true, tostring(#planned.requests))
	local sent = 0
	local jev = {
		ask = function(req)
			sent += 1
			return answering(req, function(id)
				return id == "c2" and 0.9 or 0
			end), 0.25
		end,
	}
	local asked, why, spent = output.ask(jev, planned, 0.5)
	t.equal(why, nil)
	t.equal(sent, #planned.requests)
	t.equal(spent, 0.25 * #planned.requests)
	t.equal(asked.requests, #planned.requests)
	local dropped = {}
	for chunk = 1, #planned.chunks do
		if not asked.keep[chunk] then
			table.insert(dropped, chunk)
		end
	end
	local expected = {}
	for chunk = 3, #planned.chunks - 1 do
		table.insert(expected, chunk)
	end
	t.equal(dropped, expected)
end)

-- One failed request fails the asking as a whole, with its error, and
-- every other request is still charged.
t.case("asking fails on any failure", function()
	local planned = build_plan()
	local seen = 0
	local jev = {
		ask = function(req)
			seen += 1
			if seen == 2 then
				error("Jev answered with status 503", 0)
			end
			return answering(req, function()
				return 0
			end), 0.25
		end,
	}
	local asked, why, spent = output.ask(jev, planned, 0.5)
	t.equal(asked, nil)
	t.equal(why, "Jev answered with status 503")
	t.equal(spent, 0.25 * (#planned.requests - 1))
end)

-- An answer missing, or out of range, fails the asking too.
t.case("asking fails on an answer missing or out of range", function()
	local planned = build_plan()
	local missing = {
		ask = function()
			return {}, 0
		end,
	}
	local asked, why = output.ask(missing, planned, 0.5)
	t.equal(asked, nil)
	t.equal(string.find(why, "Jev gave no yes/no answer", 1, true) ~= nil, true, why)
	local wrong = {
		ask = function(req)
			return answering(req, function()
				return 1.5
			end), 0
		end,
	}
	local again, out_of_range = output.ask(wrong, planned, 0.5)
	t.equal(again, nil)
	t.equal(string.find(out_of_range, "out of range", 1, true) ~= nil, true, out_of_range)
end)
