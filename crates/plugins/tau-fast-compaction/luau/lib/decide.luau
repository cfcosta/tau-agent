-- Asking Jev what each tool call is still worth.
--
-- Ported from `tamaratran/fast-jev-compaction` by way of
-- `joelhooks/pi-fast-jev-compaction` (`src/core/decide.ts`), with the
-- partitioned history of `tamaratran/jev-pruner` (`src/history.ts`; see
-- THIRD_PARTY_NOTICES.md).
local estimate = require("estimate")
local state = require("state")
local history = require("history")
local plan = require("plan")

local decide = {}

-- What happens to a tool call, in the order decisions escalate: the call
-- and its result stay; the call stays and its result is cut to its head
-- and a note naming the archive that holds it whole; the call and its
-- result go.
decide.ACTIONS = { keep = 1, drop_result = 2, drop_call = 3 }

-- A Noul question's JSON text, as Jev's request writes it.
function decide.noul_json(instructions, yes, no)
	local fields = { { "type", '"noul"' }, { "instructions", state.string(instructions) } }
	if yes ~= nil or no ~= nil then
		local criteria = {}
		if yes ~= nil then
			table.insert(criteria, { "true", state.string(yes) })
		end
		if no ~= nil then
			table.insert(criteria, { "false", state.string(no) })
		end
		table.insert(fields, { "criteria", state.object(criteria) })
	end
	return state.object(fields)
end

-- Questions' JSON text, by id in order, as Jev's request writes them:
-- `questions` is a list of `{ id, json }`.
function decide.questions_json(questions)
	local sorted = table.clone(questions)
	table.sort(sorted, function(a, b)
		return a.id < b.id
	end)
	local fields = {}
	for _, question in ipairs(sorted) do
		table.insert(fields, { question.id, question.json })
	end
	return state.object(fields)
end

-- The two questions for one call, each `{ id, instructions, json }`.
function decide.questions_for(call)
	local questions = {
		{
			id = "call_" .. call.id,
			instructions = "Tool call "
				.. call.id
				.. " ("
				.. call.tool
				.. ") should stay in the history: knowing this call was made, with its input, still matters for what the assistant does next",
		},
		{
			id = "result_" .. call.id,
			instructions = "The full output of tool call "
				.. call.id
				.. " ("
				.. call.tool
				.. ", "
				.. call.result_chars
				.. " chars) should stay in the history verbatim: the assistant still needs its contents and reading them back from an archive or re-running the tool would not do",
		},
	}
	for _, question in ipairs(questions) do
		question.json = decide.noul_json(question.instructions)
	end
	return questions
end

-- The decision for one call: pinned calls and calls whose full result
-- still matters are kept; a call that matters without its result keeps
-- only the result's head; anything else goes.
function decide.decide_call(call, keep_call, keep_result, threshold)
	local action
	if call.pinned or keep_result >= threshold then
		action = "keep"
	elseif keep_call >= threshold then
		action = "drop_result"
	else
		action = "drop_call"
	end
	return { call_id = call.call_id, tool = call.tool, action = action, keep_call = keep_call, keep_result = keep_result }
end

-- The JSON text of a list of call records.
local function calls_json(records)
	local texts = {}
	for _, record in ipairs(records) do
		table.insert(texts, state.call_json(record))
	end
	return state.list(texts)
end

-- Tenths of a token for a comma between items.
local COMMA_TENTHS = 9

-- The requests for a pass: each `{ state, questions, segment, asked }`,
-- `state` the state's JSON text, `questions` its questions, `asked` the
-- calls it asks about, by place in `calls`; how many segments the
-- history came in; and the largest state's tokens. When the whole
-- history fits `max_state` with the goal, every request shares that one
-- state; otherwise the history is split in order into segments, each
-- state holds one and the calls it asks about, and every call is asked
-- about against every segment. Nil and why when the goal leaves no room.
function decide.requests(entries, calls, goal, max_state, max_request)
	goal = state.goal_or_prompts(goal, entries)
	local records = state.history(entries, calls)
	local candidates = {}
	for index, call in ipairs(calls) do
		if not call.pinned then
			table.insert(candidates, index)
		end
	end
	local function state_of(history_json, listed)
		return state.state_json(goal, history_json, listed)
	end
	local function question_tenths(call)
		local questions = {}
		for _, question in ipairs(decide.questions_for(call)) do
			table.insert(questions, { id = question.id, json = question.json })
		end
		return estimate.json_tenths(decide.questions_json(questions))
	end
	local whole = state_of(state.records_json(records), nil)
	local segments, alongside
	if estimate.json_tenths(whole) <= max_state * 10 then
		segments, alongside = { records }, false
	else
		local every = {}
		for _, index in ipairs(candidates) do
			table.insert(every, state.call_record(calls[index]))
		end
		local fixed = estimate.div_ceil(estimate.json_tenths(state_of("[]", "[]")), 10)
		local all = estimate.div_ceil(estimate.json_tenths(state_of("[]", calls_json(every))), 10)
		local largest = 0
		for _, record in ipairs(every) do
			local tokens = estimate.div_ceil(estimate.json_tenths(state.call_json(record)) + COMMA_TENTHS, 10)
			if fixed + tokens <= max_state then
				largest = math.max(largest, tokens)
			end
		end
		local reserve = plan.reserve(fixed, all, largest, max_state)
		local room = max_state - reserve
		if room <= 0 then
			return nil,
				"the goal leaves no room for the history (~" .. fixed .. " of " .. max_state .. " tokens)"
		end
		local why
		segments, why = history.split(records, room)
		if segments == nil then
			return nil, why
		end
		alongside = true
	end
	local items = {}
	for _, index in ipairs(candidates) do
		table.insert(items, {
			state_tenths = alongside and estimate.json_tenths(state.call_json(state.call_record(calls[index]))) + COMMA_TENTHS
				or 0,
			question_tenths = question_tenths(calls[index]),
		})
	end
	local bases = {}
	for at, segment in ipairs(segments) do
		bases[at] = estimate.json_tenths(state_of(state.records_json(segment), alongside and "[]" or nil))
	end
	local planned = plan.plan(bases, items, max_state, max_request)
	local requests = {}
	local state_tokens = 0
	for _, one in ipairs(planned) do
		local listed = nil
		if alongside then
			local group = {}
			for _, item in ipairs(one.group) do
				table.insert(group, state.call_record(calls[candidates[item]]))
			end
			listed = calls_json(group)
		end
		local text = state_of(state.records_json(segments[one.segment]), listed)
		state_tokens = math.max(state_tokens, estimate.div_ceil(estimate.json_tenths(text), 10))
		local asked, questions = {}, {}
		for _, item in ipairs(one.batch) do
			local index = candidates[item]
			table.insert(asked, index)
			for _, question in ipairs(decide.questions_for(calls[index])) do
				table.insert(questions, question)
			end
		end
		table.insert(requests, { state = text, questions = questions, segment = one.segment, asked = asked })
	end
	return { requests = requests, segments = #segments, state_tokens = state_tokens }
end

-- The probability Jev gave the Noul `id`, or nil and why.
function decide.noul(answers, id)
	local answer = answers[id]
	if type(answer) ~= "table" or type(answer.probability) ~= "number" then
		return nil, "Jev gave no yes/no answer for `" .. id .. "`"
	end
	if answer.probability < 0 or answer.probability > 1 then
		return nil, "Jev's answer for `" .. id .. "` is out of range: " .. answer.probability
	end
	return answer.probability
end

-- Sends `requests` to Jev together, at most as many at once as Jev
-- takes: each one's answers, or nil and the first failure. What it cost.
function decide.ask(jev, requests)
	local calls = {}
	for _, request in ipairs(requests) do
		table.insert(calls, function()
			local questions = {}
			for _, question in ipairs(request.questions) do
				questions[question.id] = { kind = "noul", question = question.instructions, yes = question.yes, no = question.no }
			end
			local ok, answers, cost = pcall(jev.ask, { state_json = request.state, questions = questions })
			if not ok then
				return { error = tostring(answers) }
			end
			return { answers = answers, cost = cost or 0 }
		end)
	end
	local results = { parallel(table.unpack(calls)) }
	local spent = 0
	for _, result in ipairs(results) do
		spent += result.cost or 0
	end
	for _, result in ipairs(results) do
		if result.error ~= nil then
			return nil, result.error, spent
		end
	end
	return results, nil, spent
end

-- Decisions for every call, and how the asking went: a pinned call is
-- kept without asking; so is a call some segment was not asked about.
-- Otherwise each answer is its largest over the segments: a keep in any
-- segment keeps. Nil and why on any failure; what Jev cost either way.
function decide.decide(jev, entries, calls, settings)
	local planned, why = decide.requests(
		entries,
		calls,
		settings.goal,
		settings.max_state_tokens,
		settings.max_request_tokens
	)
	if planned == nil then
		return nil, why, 0
	end
	local results, failed, spent = decide.ask(jev, planned.requests)
	if results == nil then
		return nil, failed, spent
	end
	local tally = plan.tally(#calls, planned.segments)
	for at, request in ipairs(planned.requests) do
		for _, index in ipairs(request.asked) do
			local id = calls[index].id
			local keep_call, missing = decide.noul(results[at].answers, "call_" .. id)
			if keep_call == nil then
				return nil, missing, spent
			end
			local keep_result, wrong = decide.noul(results[at].answers, "result_" .. id)
			if keep_result == nil then
				return nil, wrong, spent
			end
			plan.record(tally, index, request.segment, { keep_call, keep_result })
		end
	end
	local decisions = {}
	for index, call in ipairs(calls) do
		local answers = plan.complete(tally, index) or { 1, 1 }
		table.insert(decisions, decide.decide_call(call, answers[1], answers[2], settings.keep_threshold))
	end
	return {
		decisions = decisions,
		requests = #planned.requests,
		segments = planned.segments,
		state_tokens = planned.state_tokens,
	},
		nil,
		spent
end

return decide
