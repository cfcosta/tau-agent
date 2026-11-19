-- What a suggestion is asked from, and what is kept of the answer (ADR
-- 0063): a run's conversation cut to what the model needs, the request
-- with its rules, and the repository's list, which an answer replaces
-- the pending part of.
local heiko = require("heiko")
local text = heiko.text

local suggest = {}

-- How many suggestions a repository keeps pending, by default and at most.
suggest.COUNT = 3
suggest.MAX_COUNT = 5

-- The longest title, in characters: the composer shows it on one line.
suggest.TITLE_CHARS = 60

-- How long a pending suggestion is shown, in seconds: a week.
suggest.SHOWN_FOR = 7 * 24 * 3600

-- What is kept of a conversation, in characters: each of the person's
-- last messages (the end, where their own words are, after what heiko
-- put before them), the last answer, and the start of each result.
suggest.ASKS = 4
suggest.ASK_CHARS = 600
suggest.ANSWER_CHARS = 1500
suggest.CALLS = 24
suggest.RESULT_CHARS = 160

-- The answer's schema, for `count` suggestions at most.
function suggest.schema(count)
	return {
		type = "object",
		properties = {
			suggestions = {
				type = "array",
				maxItems = count,
				items = {
					type = "object",
					properties = {
						title = { type = "string", minLength = 1, maxLength = suggest.TITLE_CHARS },
						prompt = { type = "string", minLength = 1 },
						why = { type = "string", minLength = 1 },
					},
					required = { "title", "prompt", "why" },
					additionalProperties = false,
				},
			},
		},
		required = { "suggestions" },
		additionalProperties = false,
	}
end

-- `value` trimmed, its last `chars` characters when it is longer.
local function tail(value, chars)
	value = text.trim(value or "")
	local codes = text.codes(value)
	if #codes <= chars then
		return value
	end
	local kept = {}
	for at = #codes - chars + 1, #codes do
		table.insert(kept, utf8.char(codes[at]))
	end
	return "…" .. table.concat(kept)
end

-- `value` trimmed, its first `chars` characters when it is longer.
local function head(value, chars)
	local kept, more = text.prefix(text.trim(value or ""), chars)
	if more then
		return kept .. "…"
	end
	return kept
end

-- What a conversation (`heiko.cmd.transcript()`'s messages) comes to:
-- `{ asks, answer, calls }`, or nil when the person said nothing in it.
function suggest.digest(messages)
	local asks, calls, answer = {}, {}, nil
	local results = {}
	for _, message in ipairs(messages or {}) do
		if message.role == "user" then
			local said = tail(message.text, suggest.ASK_CHARS)
			if said ~= "" then
				table.insert(asks, said)
			end
		elseif message.role == "assistant" then
			if message.text ~= nil and text.trim(message.text) ~= "" then
				answer = message.text
			end
			for _, call in ipairs(message.calls or {}) do
				table.insert(calls, { name = call.name })
			end
		elseif message.role == "toolResult" then
			table.insert(results, head(message.text, suggest.RESULT_CHARS))
		end
	end
	if #asks == 0 then
		return nil
	end
	-- A result answers the call before it, in order.
	for at, call in ipairs(calls) do
		call.result = results[at]
	end
	local function last(list, count)
		local out = {}
		for at = math.max(1, #list - count + 1), #list do
			table.insert(out, list[at])
		end
		return out
	end
	return {
		asks = last(asks, suggest.ASKS),
		answer = answer and head(answer, suggest.ANSWER_CHARS) or nil,
		calls = last(calls, suggest.CALLS),
	}
end

-- The request for `count` suggestions from `digest`, with the titles the
-- person already took or dismissed.
function suggest.request(digest, turned_down, count)
	local out = {
		"Suggest at most "
			.. count
			.. " next runs for a coding agent in this repository, from the conversation of the run that just ended. "
			.. "Each is something the person would plausibly ask for next. "
			.. "Base each one only on what the conversation shows, and name that in `why`. "
			.. "Do not suggest deleting, publishing, pushing, deploying or spending anything, "
			.. "and do not suggest doing again what the run already did. "
			.. "`title` is an imperative line of at most "
			.. suggest.TITLE_CHARS
			.. " characters, without quotes or trailing punctuation. "
			.. "`prompt` is the full message the person would send, in their language. "
			.. "Fewer, or none, is better than a guess.",
	}
	if #turned_down > 0 then
		table.insert(out, "\n\nThe person already took or dismissed these; do not suggest them again:")
		for _, title in ipairs(turned_down) do
			table.insert(out, "\n- " .. title)
		end
	end
	table.insert(out, "\n\nWhat the person asked, oldest first:")
	for _, ask in ipairs(digest.asks) do
		table.insert(out, "\n<ask>\n" .. ask .. "\n</ask>")
	end
	if #digest.calls > 0 then
		table.insert(out, "\n\nThe tools the run called, and the start of what each gave:")
		for _, call in ipairs(digest.calls) do
			table.insert(out, "\n- " .. call.name .. (call.result and (": " .. call.result) or ""))
		end
	end
	if digest.answer ~= nil then
		table.insert(out, "\n\nIts last answer:\n<answer>\n" .. digest.answer .. "\n</answer>")
	end
	return table.concat(out)
end

-- The titles the person took or dismissed, newest first.
function suggest.turned_down(list)
	local out = {}
	for at = #list, 1, -1 do
		local s = list[at]
		if s.status == "taken" or s.status == "dismissed" then
			table.insert(out, s.title)
		end
	end
	return out
end

-- `title` on one line, without quotes or trailing punctuation, at most
-- TITLE_CHARS: nil when nothing is left.
local OPENING = { '"', "'", "`", "“", "‘" }
local CLOSING = { '"', "'", "`", "”", "’", ".", "!", "?", ":", ";", "," }

-- `s` without the strings of `list` it starts with (`at_end` false) or
-- ends with: whole characters, never a byte of one.
local function strip(s, list, at_end)
	local cut = true
	while cut do
		cut = false
		for _, mark in ipairs(list) do
			if not at_end and string.sub(s, 1, #mark) == mark then
				s, cut = string.sub(s, #mark + 1), true
			elseif at_end and #s >= #mark and string.sub(s, -#mark) == mark then
				s, cut = string.sub(s, 1, #s - #mark), true
			end
		end
	end
	return s
end

function suggest.clean(title)
	title = text.one_line(title or "")
	title = text.trim(strip(strip(title, OPENING, false), CLOSING, true))
	if title == "" then
		return nil
	end
	local kept = text.prefix(title, suggest.TITLE_CHARS)
	return text.trim(kept)
end

-- `list` with its pending suggestions replaced by `answered`, made at
-- `at` from the run `run` by `model`: the taken and dismissed stay, at
-- most `keep` of them, oldest dropped first.
suggest.KEEP_TURNED_DOWN = 20

function suggest.merge(list, answered, run, model, at, count)
	local out = {}
	local down = 0
	for at_ = #list, 1, -1 do
		local s = list[at_]
		if s.status ~= "pending" and down < suggest.KEEP_TURNED_DOWN then
			down += 1
			table.insert(out, 1, s)
		end
	end
	local seen = {}
	for _, s in ipairs(out) do
		seen[string.lower(s.title)] = true
	end
	local added = 0
	for n, given in ipairs(answered or {}) do
		local title = suggest.clean(given.title)
		local prompt = text.trim(given.prompt or "")
		if title ~= nil and prompt ~= "" and not seen[string.lower(title)] and added < count then
			seen[string.lower(title)] = true
			added += 1
			table.insert(out, {
				id = tostring(at) .. "-" .. n,
				title = title,
				prompt = prompt,
				why = text.trim(given.why or ""),
				run = run,
				model = model,
				at = at,
				status = "pending",
			})
		end
	end
	return out, added
end

-- The pending suggestions shown at `now`, newest first.
function suggest.pending(list, now)
	local out = {}
	for at = #list, 1, -1 do
		local s = list[at]
		if s.status == "pending" and now - s.at < suggest.SHOWN_FOR then
			table.insert(out, s)
		end
	end
	return out
end

-- What the composer of a new run shows at `now`: the newest pending
-- suggestion, as `{ placeholder, prompt }`, or nil.
function suggest.hint(list, now)
	local s = suggest.pending(list, now)[1]
	if s == nil then
		return nil
	end
	return { placeholder = s.title, prompt = s.prompt }
end

-- `list` with the pending suggestion whose prompt `input` is marked
-- `status`, and whether one was: by its prompt, as Tab wrote it, or by
-- its id.
function suggest.mark(list, match, status)
	for _, s in ipairs(list) do
		if s.status == "pending" and (s.id == match or text.trim(s.prompt) == text.trim(match)) then
			s.status = status
			return list, true
		end
	end
	return list, false
end

-- The newest model of `family` the plan offers, from `env.models`, or the
-- family's name when it offers none.
function suggest.model_of(models, family)
	for _, m in ipairs(models or {}) do
		if m.family == family then
			return m.id
		end
	end
	return family
end

return suggest
