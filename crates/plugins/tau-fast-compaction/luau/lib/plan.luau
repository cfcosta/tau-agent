-- Which requests to send when the history comes in segments, and what
-- their answers add up to.
--
-- Ported from `tamaratran/jev-pruner` (`src/output.ts`,
-- `scoringRequests`; see THIRD_PARTY_NOTICES.md). Every item (an output
-- chunk, or a tool call) is asked about against every segment: the items
-- go into each segment's state in groups that fit the state budget, and
-- their questions in batches that fit the request budget beside it.
-- Counted in tenths of a token. Items are by place, from 1.
local estimate = require("estimate")

local plan = {}

-- Tokens a request takes besides its state and questions.
plan.REQUEST_OVERHEAD_TOKENS = 20

-- The tokens of a state kept free of history, given its `fixed` part,
-- `all` of it with every item, and its `largest` item that fits it at
-- all: room for every item when they take half the state or less, else
-- half; and room for the largest item beside any segment, unless that
-- leaves the history less than half of what the fixed part leaves.
function plan.reserve(fixed, all, largest, max_state)
	local half_the_rest = fixed + estimate.div_ceil(math.max(max_state - fixed, 0), 2)
	return math.max(
		math.max(math.min(all, estimate.div_ceil(max_state, 2)), math.min(fixed + largest, half_the_rest)),
		fixed
	)
end

-- The requests that ask about every item against every segment, given
-- each segment's state without items (`bases`, in tenths): items, each
-- `{ state_tenths, question_tenths }`, in groups whose state fits
-- `max_state` tokens, and questions in batches that fit `max_request`
-- tokens with the state. Each request is `{ segment, group, batch }`.
-- Requests come round robin over the segments, first requests first.
function plan.plan(bases, items, max_state, max_request)
	local max_state_tenths = max_state * 10
	local by_segment = {}
	for segment, base in ipairs(bases) do
		local groups = {}
		local group = {}
		local tenths = base
		for index, item in ipairs(items) do
			if base + item.state_tenths <= max_state_tenths then
				if #group > 0 and tenths + item.state_tenths > max_state_tenths then
					table.insert(groups, { group, tenths })
					group = {}
					tenths = base
				end
				table.insert(group, index)
				tenths += item.state_tenths
			end
		end
		if #group > 0 then
			table.insert(groups, { group, tenths })
		end
		local requests = {}
		for _, made in ipairs(groups) do
			local members, group_tenths = made[1], made[2]
			local budget = math.max(
				math.max(max_request - estimate.div_ceil(group_tenths, 10), 0) - plan.REQUEST_OVERHEAD_TOKENS,
				0
			) * 10
			local batch, used = {}, 0
			for _, index in ipairs(members) do
				local cost = items[index].question_tenths
				if cost <= budget then
					if #batch > 0 and used + cost > budget then
						table.insert(requests, { segment = segment, group = members, batch = batch })
						batch, used = {}, 0
					end
					table.insert(batch, index)
					used += cost
				end
			end
			if #batch > 0 then
				table.insert(requests, { segment = segment, group = members, batch = batch })
			end
		end
		by_segment[segment] = requests
	end
	local rounds = 0
	for _, requests in ipairs(by_segment) do
		rounds = math.max(rounds, #requests)
	end
	local out = {}
	for round = 1, rounds do
		for _, requests in ipairs(by_segment) do
			if requests[round] ~= nil then
				table.insert(out, requests[round])
			end
		end
	end
	return out
end

-- The answers about each item, per segment: `answers[item][segment]`,
-- the larger of each kept when answered twice.
function plan.tally(items, segments)
	return { items = items, segments = segments, answers = {} }
end

function plan.record(tally, item, segment, answers)
	tally.answers[item] = tally.answers[item] or {}
	local before = tally.answers[item][segment]
	if before ~= nil then
		local most = {}
		for at, answer in ipairs(answers) do
			most[at] = math.max(before[at], answer)
		end
		answers = most
	end
	tally.answers[item][segment] = answers
end

-- Each answer's largest value over the segments, once every segment
-- answered about `item`; nil while any has not.
function plan.complete(tally, item)
	local most
	for segment = 1, tally.segments do
		local answers = tally.answers[item] and tally.answers[item][segment]
		if answers == nil then
			return nil
		end
		if most == nil then
			most = table.clone(answers)
		else
			for at, answer in ipairs(answers) do
				most[at] = math.max(most[at], answer)
			end
		end
	end
	return most
end

return plan
