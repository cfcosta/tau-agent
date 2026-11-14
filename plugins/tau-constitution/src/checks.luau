-- What a check asks Jev and decides, as records (`kind`: `checked`,
-- `blocked`, `flagged`, `held`, `error`), and what they come to: a
-- run's state as its view draws it, and the counts the repository's
-- page shows.
local checks = {}

-- The name the plugin goes by, in what it tells the model.
checks.NAME = "tau-constitution"

-- What a run's checks say as it starts without a TypeSafe key.
checks.NO_KEY = "off · no TypeSafe key"

-- How much of what a rule read a note or the review queue keeps.
checks.SHOWN_CHARS = 2000

-- A number to two places.
function checks.two(x)
	return string.format("%.2f", x)
end

-- US dollars, to three places under a dollar.
function checks.usd(amount)
	local fine = string.format("%.3f", amount)
	if tonumber(fine) < 1 then
		return "$" .. fine
	end
	return string.format("$%.2f", amount)
end

-- `text` cut at `chars` characters.
function checks.cut(text, chars)
	local after = utf8.offset(text, chars + 1)
	if after == nil or after > #text then
		return text
	end
	return string.sub(text, 1, after - 1)
end

function checks.question_id(n)
	return "rule-" .. (n - 1)
end

-- What the question calls a call's fields: "the `cmd` of this
-- `exec_command` call".
function checks.what(tool, names)
	return "the `" .. table.concat(names, "` and `") .. "` of this `" .. tool .. "` call"
end

checks.FINAL_ANSWER = "this final answer"

-- The question a check asks about `rule` on `what`.
function checks.question(rule, what)
	return {
		kind = "noul",
		question = "Does " .. what .. " break this rule? Judge only what is shown.\n\nRule " .. rule.id .. ": " .. rule.text,
		yes = what .. " breaks the rule.",
		no = what .. " follows the rule, or the rule is not about it.",
	}
end

-- What Jev is asked about a call to `tool`, given the rules about it
-- (`rules.for_call`): one state for all, the tool and only the fields
-- the rules name, and a question per rule.
function checks.call_request(tool, about)
	local fields = {}
	local questions = {}
	for n, found in ipairs(about) do
		for _, name in ipairs(found.names) do
			fields[name] = found.fields[name]
		end
		questions[checks.question_id(n)] = checks.question(found.rule, checks.what(tool, found.names))
	end
	return { state = { tool = tool, arguments = fields }, questions = questions }
end

-- What Jev is asked about the final answer `answer`, given its rules.
function checks.answer_request(answer, found)
	local questions = {}
	for n, rule in ipairs(found) do
		questions[checks.question_id(n)] = checks.question(rule, checks.FINAL_ANSWER)
	end
	return { state = { final_answer = answer }, questions = questions }
end

-- Asks `jev` with `request`, of `count` questions: each rule's score in
-- order, and what it cost; else nil and why.
function checks.ask(jev, request, count)
	local ok, answers, cost = pcall(jev.ask, request)
	if not ok then
		return nil, tostring(answers)
	end
	local scores = {}
	for n = 1, count do
		local answer = answers[checks.question_id(n)]
		if type(answer) ~= "table" or type(answer.probability) ~= "number" then
			return nil, "Jev gave no answer to " .. checks.question_id(n)
		end
		scores[n] = answer.probability
	end
	return scores, cost or 0
end

-- What a check of a call shows of its fields: each value, text as it
-- is, `·` between them.
function checks.shown(fields, names)
	local parts = {}
	for _, name in ipairs(names) do
		local value = fields[name]
		if type(value) == "string" then
			table.insert(parts, value)
		else
			table.insert(parts, json.encode(value))
		end
	end
	return table.concat(parts, " · ")
end

local function score_list(rules_of, scores)
	local list = array({})
	for n, rule in ipairs(rules_of) do
		table.insert(list, { rule = rule.id, score = scores[n] })
	end
	return list
end

-- The records of a check of the call `call` (`{ id, name }`), the rules
-- about it `about` scored `scores`, what Jev cost `cost`: the check,
-- then a verdict per rule past its review mark; and what the call
-- comes to, `tau.block`'s reason when a rule is past its block mark.
function checks.of_call(call, about, scores, cost)
	local rules_of = {}
	for _, found in ipairs(about) do
		table.insert(rules_of, found.rule)
	end
	local records = {
		{ kind = "checked", call_id = call.id, tool = call.name, scores = score_list(rules_of, scores), cost = cost },
	}
	local broken = {}
	for n, rule in ipairs(rules_of) do
		if scores[n] >= rule.block then
			table.insert(broken, "rule " .. rule.id .. ' ("' .. rule.text .. '"), violation probability ' .. checks.two(scores[n]))
		end
	end
	local reason
	if #broken > 0 then
		reason = "Blocked by "
			.. checks.NAME
			.. ": this call breaks "
			.. table.concat(broken, "; and ")
			.. ". Change the call so it follows the rule, then try again."
	end
	for n, rule in ipairs(rules_of) do
		local kind
		if scores[n] >= rule.block then
			kind = "blocked"
		elseif scores[n] >= rule.review then
			kind = "flagged"
		end
		if kind then
			table.insert(records, {
				kind = kind,
				rule = rule.id,
				text = rule.text,
				score = scores[n],
				review = rule.review,
				block = rule.block,
				call_id = call.id,
				tool = call.name,
				reason = kind == "blocked" and reason or nil,
			})
		end
	end
	return records, reason
end

-- The record of a call Jev could not check, `why`, and the reason it is
-- refused when `on_error` blocks.
function checks.call_failed(call, why, on_error)
	local message = "Jev could not check the call: " .. why
	local record = { kind = "error", call_id = call.id, tool = call.name, message = message, on_error = on_error, held = false }
	if on_error == "block" then
		return record, "Blocked by " .. checks.NAME .. ": " .. message .. ", and it blocks what it cannot check. Try again."
	end
	return record, nil
end

-- The records of a check of the final answer, its rules `found` scored
-- `scores`, `holds` of `max_holds` already sent back: the check, then a
-- verdict per rule past its review mark; and the message that sends
-- the answer back, if one does. A rule broken while holds are left
-- holds it; past them, or only doubtful, a person looks.
function checks.of_answer(found, scores, cost, holds, max_holds)
	local records = { { kind = "checked", scores = score_list(found, scores), cost = cost } }
	local held = {}
	for n, rule in ipairs(found) do
		local broken = scores[n] >= rule.block
		local kind
		if broken and holds < max_holds then
			kind = "held"
		elseif scores[n] >= rule.review then
			kind = "flagged"
		end
		if kind then
			local record = {
				kind = kind,
				rule = rule.id,
				text = rule.text,
				score = scores[n],
				review = rule.review,
				block = rule.block,
			}
			if kind == "held" then
				record.reason = "Your answer breaks rule "
					.. rule.id
					.. ' ("'
					.. rule.text
					.. '"). Revise it so it follows the rule, then answer again.'
				record.hold = holds + 1
				record.max_holds = max_holds
				table.insert(held, record.reason)
			end
			table.insert(records, record)
		end
	end
	if #held == 0 then
		return records, nil
	end
	return records, table.concat(held, "\n")
end

-- The record of a final answer Jev could not check, `why`, and the
-- message that sends it back: with `on_error` block, while holds are
-- left.
function checks.answer_failed(why, on_error, holds, max_holds)
	local message = "Jev could not check the final answer: " .. why
	local held = on_error == "block" and holds < max_holds
	local record = { kind = "error", message = message, on_error = on_error, held = held }
	if not held then
		return record, nil
	end
	return record,
		checks.NAME
			.. " could not check your answer against this repository's rules ("
			.. message
			.. "), and it does not let an unchecked answer stand. Answer again."
end

-- What the checks did: calls and answers checked, questions asked,
-- checks Jev could not answer, what Jev cost, and the rules behind each
-- block, flag and hold, in order.
function checks.stats()
	return {
		calls = 0,
		answers = 0,
		questions = 0,
		failed = 0,
		cost = 0,
		blocked = array({}),
		flagged = array({}),
		held = array({}),
		flagged_answers = array({}),
	}
end

-- Counts `record` in `stats`; `answer` is the final answer a flagged
-- one is about.
function checks.count(stats, record, answer)
	local kind = record.kind
	if kind == "error" then
		stats.failed += 1
	elseif kind == "checked" then
		if record.call_id ~= nil then
			stats.calls += 1
		else
			stats.answers += 1
		end
		stats.questions += #record.scores
		stats.cost += record.cost
	elseif kind == "blocked" then
		table.insert(stats.blocked, record.rule)
	elseif kind == "flagged" then
		if record.call_id == nil then
			table.insert(stats.flagged_answers, { rule = record.rule, text = record.text, score = record.score, answer = answer or "" })
		end
		table.insert(stats.flagged, record.rule)
	elseif kind == "held" then
		table.insert(stats.held, record.rule)
		stats.max_holds = record.max_holds or stats.max_holds
	end
end

function checks.is_empty(stats)
	return stats.calls == 0 and stats.answers == 0
end

-- The checks in a line: `1 blocked · 1 flagged`, or `2 checks, all
-- clear`.
function checks.summary(stats)
	local parts = {}
	for _, what in ipairs({ "blocked", "flagged", "held" }) do
		if #stats[what] > 0 then
			table.insert(parts, #stats[what] .. " " .. what)
		end
	end
	if #parts > 0 then
		return table.concat(parts, " · ")
	end
	local n = stats.calls + stats.answers
	return n .. " " .. (n == 1 and "check" or "checks") .. ", all clear"
end

-- What a run's checks say as it starts: off without a key, else how
-- many rules they watch; `rules` is nil when they cannot be read.
function checks.starting(jev, count)
	if not jev then
		return checks.NO_KEY
	elseif count == nil then
		return "the rules cannot be read"
	elseif count == 0 then
		return "no rules"
	elseif count == 1 then
		return "watching 1 rule"
	end
	return "watching " .. count .. " rules"
end

-- The run's state, as each hook finds it: `starting`, what it says
-- before anything is checked; `stats`; `calls`, what the checks made of
-- each call by its id (`{ tool, shown, scores, verdict }`); `notes` by
-- key; and `said`, the messages it sent back.
function checks.state(state)
	state.stats = state.stats or checks.stats()
	state.calls = state.calls or {}
	state.notes = state.notes or {}
	state.said = state.said or {}
	return state
end

local function note(state, text, detail, tone)
	local n = 0
	for _ in pairs(state.notes) do
		n += 1
	end
	state.notes["c" .. n] = { text = text, detail = detail, tone = tone }
end

-- How grave a call's verdict is: a block refused it.
local SEVERITY = { flagged = 1, blocked = 2 }

-- The call `id` as the state keeps it.
local function call_of(state, id, tool, shown)
	local call = state.calls[id]
	if call == nil then
		call = { tool = tool or "", shown = shown or "", scores = array({}) }
		state.calls[id] = call
	end
	return call
end

-- Folds `record` into the run's `state`, as its view draws it: the
-- counts, each call's scores and verdict, and a note for each check Jev
-- could not answer and each verdict on the final answer. `answer` is
-- the final answer, and `shown` what a call's rules read.
function checks.fold(state, record, answer, shown)
	checks.count(state.stats, record, answer)
	local kind = record.kind
	if kind == "error" then
		local on_call = record.call_id ~= nil
		local detail
		if record.on_error == "block" and on_call then
			detail = "not checked · blocked"
		elseif record.on_error == "block" and record.held then
			detail = "not checked · sent back"
		elseif on_call then
			detail = "not checked · ran"
		else
			detail = "not checked · the answer stands"
		end
		note(state, record.message, detail, "bad")
		return
	end
	if kind == "checked" then
		if record.call_id ~= nil then
			call_of(state, record.call_id, record.tool, shown).scores = record.scores
		end
		return
	end
	if record.call_id ~= nil then
		local call = call_of(state, record.call_id, record.tool, shown)
		-- Rules read a call one after another: the call was refused if any
		-- blocked it, so the gravest verdict stands, the first of those.
		if call.verdict ~= nil and (SEVERITY[call.verdict.kind] or 0) >= (SEVERITY[kind] or 0) then
			return
		end
		call.verdict = {
			kind = kind,
			rule = record.rule,
			score = record.score,
			review = record.review,
			block = record.block,
			reason = record.reason or record.text,
		}
		return
	end
	local said
	if kind == "held" then
		said = "held the stop: the answer breaks "
	else
		said = "flagged the answer for review: "
	end
	local detail
	if record.hold ~= nil and record.max_holds ~= nil then
		detail = "before_stop · continuation " .. record.hold .. " / " .. record.max_holds
	else
		detail = "before_stop · " .. checks.two(record.score)
	end
	note(state, said .. record.rule .. ' ("' .. record.text .. '")', detail, "warn")
end

-- The run's status line, and its tone.
function checks.status(state)
	local stats = state.stats
	if checks.is_empty(stats) then
		if state.starting == nil then
			return nil
		end
		return state.starting, "neutral"
	end
	return checks.summary(stats), #stats.blocked > 0 and "bad" or "neutral"
end

-- What a repository's runs say about its rules, from each run's stats:
-- runs, checks, failures, blocks, flags, runs held, what Jev cost, and
-- per rule `{ blocked, flagged, held }`.
function checks.totals(runs)
	local totals = { runs = 0, checked = 0, failed = 0, blocked = 0, flagged = 0, held_runs = 0, cost = 0, per_rule = {} }
	for _, stats in ipairs(runs) do
		totals.runs += 1
		totals.checked += stats.calls + stats.answers
		totals.failed += stats.failed
		totals.blocked += #stats.blocked
		totals.flagged += #stats.flagged
		totals.cost += stats.cost
		if #stats.held > 0 then
			totals.held_runs += 1
		end
		for slot, list in ipairs({ stats.blocked, stats.flagged, stats.held }) do
			for _, rule in ipairs(list) do
				local counts = totals.per_rule[rule] or { 0, 0, 0 }
				counts[slot] += 1
				totals.per_rule[rule] = counts
			end
		end
	end
	return totals
end

return checks
