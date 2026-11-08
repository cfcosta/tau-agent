-- The history stage's pure parts, over drawn transcripts: the ledger and
-- what it makes of a transcript, the requests that ask Jev about every
-- call, the goal, the history, and the archives of cut results.
local tau = require("tau")
local t = tau.test
local g = t.gen
local estimate = require("estimate")
local state = require("state")
local decide = require("decide")
local ledger = require("ledger")
local plan = require("plan")

local TRUNCATED = "[fast-compaction truncated "

local function contains(text, part)
	return string.find(text, part, 1, true) ~= nil
end

local function blank(text)
	return string.find(text, "%S") == nil
end

-- `chars` characters of `pieces` repeated, cut with `utf8.offset`.
local function cycle(pieces, chars)
	if chars == 0 then
		return ""
	end
	local text = string.rep(table.concat(pieces), chars // #pieces + 1)
	return string.sub(text, 1, utf8.offset(text, chars + 1) - 1)
end

-- Text of a drawn length up to `max` characters, made of a drawn
-- pattern of `alphabet`'s characters.
local function sized(alphabet, max, min)
	return g.composite(function(draw)
		local chars = draw(g.integers({ min = min or 0, max = max }))
		local pattern = draw(g.lists(g.sampled_from(alphabet), { min = 1, max = 8 }))
		return cycle(pattern, chars)
	end)
end

-- Whatever a result holds: a text of up to 3000 characters, sometimes
-- one already cut, and sometimes an image.
local RESULT_ALPHABET = { "a", "b", " ", "\n", ".", "é", "日", "🦀" }
local result_content = g.composite(function(draw)
	local images = draw(g.integers({ min = 0, max = 5 })) == 0 and 1 or nil
	local text
	if draw(g.integers({ min = 0, max = 5 })) == 0 then
		text = TRUNCATED .. draw(sized(RESULT_ALPHABET, 200))
	else
		text = draw(sized(RESULT_ALPHABET, 3000))
	end
	return { text = text, images = images }
end)

local LONG_ALPHABET = { "a", "b", "c", "d", "e", "f", "g", "h", " ", "i", "j", "\n", ".", ",", "1", "é" }
local long_text = sized(LONG_ALPHABET, 1500)
local short_text = sized({ "a", "b", " ", "\n", "." }, 40)

-- A transcript as `rewrite_context` reads it: turns of a prompt, then an
-- answer that may think, say words and call tools, then the calls'
-- results. `words` draws prompts and answers; `input`, when given, a
-- text each call's input holds; `result(index)` each result's
-- `{ text, images }`.
local function transcript(words, input, result)
	return g.composite(function(draw)
		local messages = {}
		local calls_made = 0
		local turns = draw(g.integers({ min = 1, max = 5 }))
		for _ = 1, turns do
			table.insert(messages, { role = "user", text = draw(words) })
			local calls = {}
			for _ = 1, draw(g.integers({ min = 0, max = 3 })) do
				calls_made += 1
				local args = { path = "f" .. calls_made .. ".rs" }
				if input ~= nil then
					args.long = draw(input)
				end
				table.insert(calls, {
					id = "call-" .. calls_made,
					name = draw(g.sampled_from({ "read", "exec_command" })),
					args = args,
					order = { "path", "long" },
					json = json.encode(args),
				})
			end
			local said = draw(g.booleans()) and draw(words) or ""
			local answer = { role = "assistant", text = said, calls = calls }
			if draw(g.booleans({ p = 0.25 })) then
				answer.parts = { { thinking = "let me see" } }
				if said ~= "" then
					table.insert(answer.parts, { text = said })
				end
			end
			table.insert(messages, answer)
			for _, call in ipairs(calls) do
				local message = {
					role = "toolResult",
					call = call.id,
					name = call.name,
					ok = draw(g.booleans()),
				}
				local content = result(#messages + 1, draw)
				message.text = content.text
				message.images = content.images
				table.insert(messages, message)
			end
		end
		for index, message in ipairs(messages) do
			message.index = index
			message.tokens = 1
		end
		return messages
	end)
end

local with_long_results = transcript(short_text, nil, function(_, draw)
	return draw(result_content)
end)

-- Prompts, answers and inputs of long texts, and results that say only
-- a marker nothing else can say.
local with_long_texts = transcript(long_text, long_text, function(index)
	return { text = "OUTPUT-MARKER-" .. index }
end)

local function call_ids(messages)
	local ids = {}
	for _, message in ipairs(messages) do
		if message.role == "assistant" then
			for _, call in ipairs(message.calls) do
				table.insert(ids, call.id)
			end
		end
	end
	return ids
end

local ACTIONS = { "keep", "drop_result", "drop_call" }

-- A ledger with a drawn action for some of the transcript's calls.
local function ledger_for(messages, draw)
	local decisions = {}
	for _, id in ipairs(call_ids(messages)) do
		if draw(g.booleans()) then
			table.insert(decisions, {
				call_id = id,
				tool = "t",
				action = draw(g.sampled_from(ACTIONS)),
				keep_call = 0.5,
				keep_result = 0.5,
			})
		end
	end
	return ledger.of(decisions)
end

local function copy(message)
	local out = table.clone(message)
	if message.calls ~= nil then
		out.calls = table.clone(message.calls)
	end
	return out
end

-- What the host makes of `messages` with the ledger's `spec`
-- (tau-luau-plugins-host's `rebuild`): dropped calls go with their
-- results, an answer that lost a call and has neither words nor a call
-- left goes too, each of `texts` replaces its result's text, images
-- gone, and only `keep` stays.
local function rebuilt(messages, spec)
	local dropped, texts, kept = {}, {}, {}
	for _, id in ipairs(spec.drop_calls) do
		dropped[id] = true
	end
	for _, change in ipairs(spec.texts) do
		texts[change.index] = change.text
	end
	for _, index in ipairs(spec.keep) do
		kept[index] = true
	end
	local out = {}
	for index, message in ipairs(messages) do
		local made = copy(message)
		local gone = not kept[index]
		if message.role == "assistant" then
			made.calls = {}
			for _, call in ipairs(message.calls) do
				if not dropped[call.id] then
					table.insert(made.calls, call)
				end
			end
			if #made.calls < #message.calls and #made.calls == 0 and message.text == "" then
				gone = true
			end
		elseif message.role == "toolResult" and dropped[message.call] then
			gone = true
		end
		if texts[index] ~= nil then
			made.text = texts[index]
			made.images = nil
		end
		if not gone then
			table.insert(out, made)
		end
	end
	return out
end

-- What a ledger makes of a transcript, stated from the old Rust
-- ledger's documentation: calls dropped with their results; an answer
-- losing a call and left with neither words nor calls goes; a dropped
-- result not already cut that holds an image or runs past `head + 120`
-- characters becomes its first `head` characters, a new line when there
-- are any, and a note of what was cut; everything else stays.
local function expected_apply(by_id, messages, head)
	local function action(id)
		return by_id[id] and by_id[id].action or "keep"
	end
	local surviving, out = {}, {}
	for _, message in ipairs(messages) do
		if message.role == "assistant" then
			local made = copy(message)
			made.calls = {}
			for _, call in ipairs(message.calls) do
				if action(call.id) ~= "drop_call" then
					table.insert(made.calls, call)
					surviving[call.id] = true
				end
			end
			local lost = #made.calls < #message.calls
			if not (lost and message.text == "" and #made.calls == 0) then
				table.insert(out, made)
			end
		elseif message.role == "toolResult" then
			local decided = action(message.call)
			if surviving[message.call] and decided ~= "drop_call" then
				local chars = utf8.len(message.text)
				if
					decided == "drop_result"
					and not contains(message.text, TRUNCATED)
					and ((message.images or 0) > 0 or chars > head + 120)
				then
					local kept = string.sub(message.text, 1, (utf8.offset(message.text, head + 1) or #message.text + 1) - 1)
					local made = copy(message)
					made.images = nil
					made.text = kept
						.. (head > 0 and "\n" or "")
						.. TRUNCATED
						.. math.max(chars - head, 0)
						.. " chars of this tool result"
						.. (message.ok == false and " (error)" or "")
						.. "; re-run the tool if needed]"
					table.insert(out, made)
				else
					table.insert(out, copy(message))
				end
			end
		else
			table.insert(out, copy(message))
		end
	end
	return out
end

local function users(messages)
	local out = {}
	for _, message in ipairs(messages) do
		if message.role == "user" then
			table.insert(out, message)
		end
	end
	return out
end

-- The ledger's rewrite keeps every message and names what goes: the
-- calls it drops (their results and an answer left empty go with them,
-- the host's job) and the results it cuts. What the host makes of it is
-- what the Rust ledger made: long or image-holding dropped results cut
-- to their head and a note, short ones and ones already cut kept, a
-- thinking-only answer kept. An empty ledger changes nothing, applying
-- twice changes nothing more, the person's messages stay as they were,
-- and every surviving call's results follow it.
t.property("the ledger keeps the transcript well formed", function(draw)
	local messages = draw(with_long_results)
	local by_id = ledger_for(messages, draw)
	local head = draw(g.integers({ min = 0, max = 200 }))
	local spec = ledger.apply(by_id, messages, head)
	t.equal(#spec.keep, #messages, "every message is kept, the host drops the rest")
	local applied = rebuilt(messages, spec)
	t.equal(applied, expected_apply(by_id, messages, head))

	local empty = ledger.apply(ledger.of({}), messages, head)
	t.equal(#empty.drop_calls + #empty.texts, 0)
	t.equal(rebuilt(messages, empty), messages)
	local again = ledger.apply(by_id, applied, head)
	t.equal(#again.drop_calls + #again.texts, 0, "idempotent")
	t.equal(rebuilt(applied, again), applied, "idempotent")
	t.equal(users(applied), users(messages))

	for _, id in ipairs(call_ids(applied)) do
		t.equal(ledger.action(by_id, id) ~= "drop_call", true, id .. " was dropped")
	end
	for index, message in ipairs(applied) do
		if message.role == "assistant" then
			local calls, results = {}, {}
			for _, call in ipairs(message.calls) do
				table.insert(calls, call.id)
			end
			local at = index + 1
			while applied[at] ~= nil and applied[at].role == "toolResult" do
				table.insert(results, applied[at].call)
				at += 1
			end
			t.equal(results, calls, "results after message " .. index)
		end
	end
end, { cases = 200 })

-- The ledger's rewrite of a transcript, as a spec: the answer whose only
-- call goes stays in `keep`, its call named in `drop_calls`; a result
-- cut has its index and its new text in `texts`.
t.case("the rewrite names what goes and what is cut", function()
	local messages = {
		{ index = 1, role = "user", text = "go" },
		{ index = 2, role = "assistant", text = "", calls = { { id = "a", name = "read", args = {}, json = "{}" } } },
		{ index = 3, role = "toolResult", call = "a", name = "read", ok = true, text = "short" },
		{ index = 4, role = "assistant", text = "", calls = { { id = "b", name = "read", args = {}, json = "{}" } } },
		{ index = 5, role = "toolResult", call = "b", name = "read", ok = false, text = string.rep("x", 200) },
	}
	local by_id = ledger.of({
		{ call_id = "a", tool = "read", action = "drop_call" },
		{ call_id = "b", tool = "read", action = "drop_result" },
	})
	local spec = ledger.apply(by_id, messages, 10)
	t.equal(spec.keep, { 1, 2, 3, 4, 5 })
	t.equal(spec.drop_calls, { "a" })
	t.equal(spec.texts, {
		{
			index = 5,
			text = "xxxxxxxxxx\n" .. TRUNCATED .. "190 chars of this tool result (error); re-run the tool if needed]",
		},
	})
end)

local RANK = { keep = 1, drop_result = 2, drop_call = 3 }

local decision_drawn = g.composite(function(draw)
	return {
		call_id = draw(g.sampled_from({ "a", "b", "c" })),
		tool = "t",
		action = draw(g.sampled_from(ACTIONS)),
		keep_call = draw(g.sampled_from({ 0, 0.25, 0.5, 1 })),
		keep_result = 0.5,
	}
end)

-- After any merges, each call holds the first of its decisions with the
-- harshest action: a milder or equal later one changes nothing,
-- keep_call included; and the list comes in id order.
t.property("decisions only escalate", function(draw)
	local batches = draw(g.lists(g.lists(decision_drawn, { max = 4 }), { min = 1, max = 4 }))
	local by_id = ledger.of(batches[1])
	for at = 2, #batches do
		ledger.merge(by_id, batches[at])
	end
	local model = {}
	for _, batch in ipairs(batches) do
		for _, decision in ipairs(batch) do
			local held = model[decision.call_id]
			if held == nil or RANK[held.action] < RANK[decision.action] then
				model[decision.call_id] = decision
			end
		end
	end
	local expected = {}
	for _, id in ipairs({ "a", "b", "c" }) do
		if model[id] ~= nil then
			table.insert(expected, model[id])
		end
	end
	t.equal(ledger.list(by_id), expected)
end, { cases = 200 })

local GOAL = "fix the bug in a.rs"

local function dedup(list)
	local out = {}
	for _, value in ipairs(list) do
		if out[#out] ~= value then
			table.insert(out, value)
		end
	end
	return out
end

-- Over any history and budgets: every state fits `max_state` and every
-- request, with its questions, `max_request`. When the history fits
-- whole, one state holds it without a list of calls; otherwise each
-- state holds a segment, lists the calls it asks about, and the
-- segments hold the whole history once, in order. No tool output ever
-- reaches a state. Every unpinned call is asked about once per segment,
-- unless it is too big to fit beside some, and no pinned one is.
t.property("the history stage asks about every call in every segment", function(draw)
	local messages = draw(with_long_texts)
	local preserve = draw(g.integers({ min = 1, max = 4 }))
	local max_state = draw(g.integers({ min = 1500, max = 8000 }))
	local max_request = max_state + draw(g.integers({ min = 400, max = 3000 }))
	local entries = state.entries(messages)
	local calls = state.calls(entries, preserve)
	local planned, why = decide.requests(entries, calls, GOAL, max_state, max_request)
	t.equal(why, nil)
	local fixed = estimate.state_tokens(state.state_json(GOAL, "[]", "[]"))
	local function fits_anywhere(call)
		local record = state.call_json(state.call_record(call))
		return estimate.div_ceil(estimate.json_tenths(record) + 9, 10)
			<= estimate.div_ceil(math.max(max_state - fixed, 0), 2)
	end
	t.note(planned.segments .. " segments")
	local asked, by_segment = {}, {}
	for _, request in ipairs(planned.requests) do
		local tokens = estimate.state_tokens(request.state)
		t.equal(tokens <= max_state, true, tokens .. " > " .. max_state)
		local questions = decide.questions_json(request.questions)
		t.equal(tokens + estimate.state_tokens(questions) + plan.REQUEST_OVERHEAD_TOKENS <= max_request, true)
		t.equal(contains(request.state, "OUTPUT-MARKER-"), false, "a tool output reached the state")
		local decoded = json.decode(request.state)
		t.equal(decoded.calls == nil, planned.segments == 1)
		for _, index in ipairs(request.asked) do
			local call = calls[index]
			t.equal(call.pinned, false)
			asked[index] = asked[index] or {}
			t.equal(asked[index][request.segment], nil, "asked twice in a segment")
			asked[index][request.segment] = true
			if planned.segments > 1 then
				local listed = false
				for _, record in ipairs(decoded.calls) do
					listed = listed or record.id == call.id
				end
				t.equal(listed, true, call.id .. " asked but not listed")
			end
		end
		local seen = {}
		for _, record in ipairs(decoded.history) do
			table.insert(seen, record.i)
		end
		by_segment[request.segment] = dedup(seen)
	end
	for index, call in ipairs(calls) do
		local count = 0
		for _ in pairs(asked[index] or {}) do
			count += 1
		end
		if call.pinned then
			t.equal(count, 0)
		elseif fits_anywhere(call) then
			t.equal(count, planned.segments, call.id)
		end
	end
	local all, complete = {}, true
	for segment = 1, planned.segments do
		if by_segment[segment] == nil then
			complete = false
		else
			for _, i in ipairs(by_segment[segment]) do
				table.insert(all, i)
			end
		end
	end
	if complete then
		local whole = {}
		for _, record in ipairs(state.history(entries, calls)) do
			table.insert(whole, record.i)
		end
		t.equal(dedup(all), whole)
	end
end, { cases = 100 })

-- Text whose lengths gather around the 500-character cut.
local PROMPT_ALPHABET = { "a", "b", " ", ".", "\n", "é", "日", "🦀" }
local prompt_text = g.composite(function(draw)
	local chars = draw(g.one_of({
		g.sampled_from({ 1, 499, 500, 501, 502 }),
		g.integers({ min = 1, max = 700 }),
	}))
	local pattern = draw(g.lists(g.sampled_from(PROMPT_ALPHABET), { min = 1, max = 8 }))
	return cycle(pattern, chars)
end)

-- A message to build a goal from: a prompt (sometimes blank), the
-- assistant's words, or a tool result.
local said = g.composite(function(draw)
	local kind = draw(g.integers({ min = 0, max = 4 }))
	if kind == 0 then
		return { role = "user", text = draw(sized({ " ", "\t", "\n" }, 3)) }
	elseif kind <= 2 then
		return { role = "user", text = draw(prompt_text) }
	elseif kind == 3 then
		return { role = "assistant", text = draw(prompt_text), calls = {} }
	end
	return { role = "toolResult", call = "c", name = "t", ok = true, text = draw(prompt_text) }
end)

-- Without a goal, the state's is the person's last three prompts, each
-- cut to 500 characters (499 and an ellipsis), not bytes, joined by new
-- lines; tool results and the assistant's words are not prompts, and
-- neither is a blank one. A goal that is set wins; an empty one is none.
t.property("the goal is the person's last three prompts", function(draw)
	local messages = draw(g.lists(said, { max = 8 }))
	local prompts = {}
	for _, message in ipairs(messages) do
		if message.role == "user" and not blank(message.text) then
			table.insert(prompts, message.text)
		end
	end
	local kept = {}
	for at = math.max(#prompts - 2, 1), #prompts do
		local prompt = prompts[at]
		if utf8.len(prompt) <= 500 then
			table.insert(kept, prompt)
		else
			table.insert(kept, string.sub(prompt, 1, utf8.offset(prompt, 500) - 1) .. "…")
		end
	end
	local goal = table.concat(kept, "\n")
	local entries = state.entries(messages)
	t.equal(state.goal_from(entries), goal)
	t.equal(state.goal_or_prompts(nil, entries), goal)
	t.equal(state.goal_or_prompts("", entries), goal)
	t.equal(state.goal_or_prompts("ship it", entries), "ship it")
end, { cases = 300 })

-- The history keeps every message with words or with tool calls that
-- have results, whole and in order, a long prompt included; a blank
-- message and a tool result are left out. Each call comes with the
-- message that made it, named `t1`, `t2`… in order, with its input as
-- JSON and a note of its result's status and size, never the result.
t.property("the history keeps every message with words or calls whole", function(draw)
	local messages = draw(with_long_texts)
	local entries = state.entries(messages)
	local calls = state.calls(entries, 1)
	local records = state.history(entries, calls)
	local results = {}
	for _, message in ipairs(messages) do
		if message.role == "toolResult" then
			results[message.call] = message
		end
	end
	local named = 0
	local expected = {}
	for i, message in ipairs(messages) do
		if message.role == "user" then
			if not blank(message.text) then
				table.insert(expected, { i = i - 1, role = "user", text = message.text, tool_calls = {}, tool_results = {} })
			end
		elseif message.role == "assistant" then
			local made = {}
			for _, call in ipairs(message.calls) do
				local result = results[call.id]
				if result ~= nil then
					named += 1
					table.insert(made, {
						id = "t" .. named,
						tool = call.name,
						input = call.json,
						result = (result.ok == false and "error" or "ok") .. ", " .. utf8.len(result.text) .. " chars (omitted)",
					})
				end
			end
			local text = entries[i].text
			if not blank(text) or #made > 0 then
				table.insert(expected, { i = i - 1, role = "assistant", text = text, tool_calls = made, tool_results = {} })
			end
		end
	end
	t.equal(records, expected)
end, { cases = 200 })

-- Every result a ledger cuts names, in its note, an archive that holds
-- the result's text exactly; each archive belongs to one cut result,
-- and a second assignment finds nothing new to archive.
t.property("a cut result names an archive holding it", function(draw)
	local messages = draw(with_long_results)
	local by_id = ledger_for(messages, draw)
	local head = draw(g.integers({ min = 0, max = 200 }))
	local made = 0
	local function path(kind)
		made += 1
		return "/archive/tau-" .. kind .. "-" .. made .. ".txt"
	end
	local files = {}
	local archives = ledger.assign_archives(by_id, messages, head, path)
	for _, archive in ipairs(archives) do
		t.equal(files[archive.path], nil, archive.path .. " assigned twice")
		files[archive.path] = archive.text
	end
	t.equal(#ledger.assign_archives(by_id, messages, head, path), 0)
	local spec = ledger.apply(by_id, messages, head)
	local named = {}
	local count = 0
	for _, change in ipairs(spec.texts) do
		local original = messages[change.index].text
		local text = change.text
		t.equal(text ~= original, true)
		local marker = "; full result: "
		local at, last = 1, nil
		while true do
			local found = string.find(text, marker, at, true)
			if found == nil then
				break
			end
			last = found
			at = found + 1
		end
		t.equal(last ~= nil, true, "no archive named in " .. text)
		local rest = string.sub(text, last + #marker)
		local ending = " (read or grep it if needed)]"
		t.equal(string.sub(rest, -#ending), ending)
		local named_path = string.sub(rest, 1, #rest - #ending)
		t.equal(files[named_path], original)
		t.equal(named[named_path], nil, named_path .. " named twice")
		named[named_path] = true
		count += 1
	end
	t.equal(count, #archives)
end, { cases = 100 })
