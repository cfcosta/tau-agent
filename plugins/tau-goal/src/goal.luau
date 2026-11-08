-- What tau-goal knows of goals (docs/reference/goal.md): how `/goal`
-- reads, the input the model gets for a goal, and a conversation's goal
-- as its checks leave it, with the notes, lines and badges that say it.
local goal = {}

-- Continuations a goal allows when `/goal` does not say.
goal.DEFAULT_CONTINUATIONS = 10

-- US dollars a goal may cost when `/goal` does not say: the run's turns
-- and the checks, from when it was set.
goal.DEFAULT_BUDGET = 2.0

-- The probability at or past which the goal counts as met.
goal.MET_AT = 0.7

-- How many of the latest tool results Jev sees.
goal.EVIDENCE = 6

-- How much of a tool result Jev sees: its end, where results and
-- summaries are.
goal.OUTPUT_CHARS = 1500

-- How the model is told about its goal, after the condition.
goal.INSTRUCTIONS = "This is your goal. Keep working until it holds. Each time you stop, tau-goal checks it against your last answer and your recent tool results, and sends you back if it does not hold yet, so run what proves it before you stop."

-- How a continuation starts, so interfaces can tell it from a person's
-- message.
goal.CONTINUATION_PREFIX = "tau-goal: "

-- What a person sends to go on toward the goal.
goal.KEEP_GOING = "Keep going toward the goal."

-- What the model reads when a message clears the goal.
goal.CLEARED = "The goal is cleared."

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

local function is_space(c)
	return SPACE[c] == true
end

-- `text` from its first character that is not white space.
local function trim_start(text)
	for at, c in utf8.codes(text) do
		if not is_space(c) then
			return string.sub(text, at)
		end
	end
	return ""
end

-- `text` without white space at either end.
local function trim(text)
	text = trim_start(text)
	local last = 0
	for at, c in utf8.codes(text) do
		if not is_space(c) then
			last = utf8.offset(text, 2, at) - 1
		end
	end
	return string.sub(text, 1, last)
end
goal.trim = trim

-- The first word of `text`, up to white space, and what follows it, its
-- leading white space gone.
local function next_word(text)
	for at, c in utf8.codes(text) do
		if is_space(c) then
			return string.sub(text, 1, at - 1), trim_start(string.sub(text, at))
		end
	end
	return text, ""
end

-- The words of `text`, between runs of white space.
local function words(text)
	local list = {}
	local rest = trim_start(text)
	while rest ~= "" do
		local word
		word, rest = next_word(rest)
		table.insert(list, word)
	end
	return list
end

-- Whether `text` starts with white space.
local function starts_with_space(text)
	for _, c in utf8.codes(text) do
		return is_space(c)
	end
	return false
end

local ESCAPED = { ['"'] = '"', ["\\"] = "\\", n = "\n", r = "\r" }

-- What a quoted string says, when `text` is exactly one: `"`, then
-- characters with `\"`, `\\`, `\n` and `\r` escaped, then `"`.
local function unquote(text)
	if string.sub(text, 1, 1) ~= '"' then
		return nil
	end
	local out = {}
	local at = 2
	while at <= #text do
		local c = string.sub(text, at, at)
		if c == '"' then
			return at == #text and table.concat(out) or nil
		elseif c == "\\" then
			local escaped = ESCAPED[string.sub(text, at + 1, at + 1)]
			if escaped == nil then
				return nil
			end
			table.insert(out, escaped)
			at += 2
		else
			table.insert(out, c)
			at += 1
		end
	end
	return nil
end

-- `condition` as a quoted string `unquote` reads back.
local function quote(condition)
	local escaped = string.gsub(condition, '[\\"\n\r]', { ['"'] = '\\"', ["\\"] = "\\\\", ["\n"] = "\\n", ["\r"] = "\\r" })
	return '"' .. escaped .. '"'
end
goal.quote = quote

-- A whole number of continuations, as Rust's `u32` reads one.
local function continuations_of(text)
	if not string.match(text, "^%+?%d+$") then
		return nil
	end
	local n = tonumber((string.gsub(text, "^%+", "")))
	if n == nil or n > 4294967295 then
		return nil
	end
	return n
end

-- A budget: a finite amount of at least zero, with a `$` or not.
local function budget_of(text)
	text = string.gsub(text, "^%$+", "")
	local number = string.match(text, "^[+-]?%d+%.?%d*[eE][+-]?%d+$")
		or string.match(text, "^[+-]?%d+%.?%d*$")
		or string.match(text, "^[+-]?%.%d+[eE][+-]?%d+$")
		or string.match(text, "^[+-]?%.%d+$")
	if number == nil then
		return nil
	end
	local amount = tonumber(number)
	if amount == nil or amount ~= amount or amount == math.huge or amount < 0 then
		return nil
	end
	return amount
end

-- Reads `/goal [--continuations N] [--budget USD] <condition>` or `/goal
-- clear`: `{ kind = "set", condition, continuations, budget }`, `{ kind
-- = "clear" }`, or nil for anything else, a budget that is not a finite
-- amount of at least zero included.
--
-- A condition is its words joined by single spaces, unless it is one
-- quoted string, `"..."`: that is read exactly as written, with `\"`,
-- `\\`, `\n` and `\r` for a quote, a backslash, a line feed and a
-- carriage return. `/goal "clear"` sets the goal `clear`.
function goal.parse(input)
	local rest = trim(input)
	if string.sub(rest, 1, 5) ~= "/goal" then
		return nil
	end
	rest = string.sub(rest, 6)
	if rest ~= "" and not starts_with_space(rest) then
		return nil
	end
	rest = trim_start(rest)
	local continuations = goal.DEFAULT_CONTINUATIONS
	local budget = goal.DEFAULT_BUDGET
	while true do
		local word, after = next_word(rest)
		if word == "--continuations" then
			local n
			n, rest = next_word(after)
			continuations = continuations_of(n)
			if continuations == nil then
				return nil
			end
		elseif word == "--budget" then
			local usd
			usd, rest = next_word(after)
			budget = budget_of(usd)
			if budget == nil then
				return nil
			end
		else
			break
		end
	end
	local quoted = unquote(rest)
	if quoted ~= nil then
		if trim(quoted) == "" then
			return nil
		end
		return { kind = "set", condition = quoted, continuations = continuations, budget = budget }
	end
	local condition = table.concat(words(rest), " ")
	if condition == "" then
		return nil
	elseif condition == "clear" then
		return { kind = "clear" }
	end
	return { kind = "set", condition = condition, continuations = continuations, budget = budget }
end

-- The first line of `text`, as Rust's `lines` reads it, if it has one.
local function first_line(text)
	if text == "" then
		return nil
	end
	local line = string.match(text, "^([^\n]*)")
	return (string.gsub(line, "\r$", ""))
end

-- The condition of a message that set a goal: `/goal ...`, as typed or
-- as `set_input` wrote it.
function goal.set_message(text)
	local line = first_line(text)
	local command = line and goal.parse(line)
	return command and command.kind == "set" and command.condition or nil
end

-- The input the model gets for a goal: the condition, then how goals
-- work. The condition is written as is when it reads back that way, and
-- quoted otherwise: when it would read as `clear` or as limits, spans
-- lines, has runs of white space or is itself one quoted string.
function goal.set_input(condition)
	local plain = "/goal " .. condition .. "\n\n" .. goal.INSTRUCTIONS
	if goal.set_message(plain) == condition then
		return plain
	end
	return "/goal " .. quote(condition) .. "\n\n" .. goal.INSTRUCTIONS
end

-- The last `most` characters of `text`, after an ellipsis when it was
-- longer.
function goal.tail(text, most)
	local count = utf8.len(text) or #text
	if count <= most then
		return text
	end
	return "…" .. string.sub(text, utf8.offset(text, count - most + 1))
end

-- An amount in US dollars, as tau's interface writes one: three places
-- under a dollar, else two.
function goal.usd(amount)
	local fine = string.format("%.3f", amount)
	if tonumber(fine) < 1 then
		return "$" .. fine
	end
	return string.format("$%.2f", amount)
end

-- The plugin's state, as each hook finds it: `goal`, the conversation's
-- (`condition`, `max_continuations`, `budget`, `status` one of
-- "active", "paused", "met" and "stopped", `exhausted` when stopped,
-- `continuations` used, `spent`, `checks` oldest first, `error`); its
-- `notes` in the transcript by key; `held`, the continuations a check's
-- note already says; and `said`, how the person's messages it knows
-- draw, by their text.
function goal.state(state)
	state.notes = state.notes or {}
	state.held = state.held or {}
	state.said = state.said or {}
	return state
end

local function count(map)
	local n = 0
	for _ in pairs(map) do
		n += 1
	end
	return n
end

-- Puts a note in the transcript.
local function note(state, text, detail, tone)
	state.notes["g" .. count(state.notes)] = { text = text, detail = detail, tone = tone }
end

-- Sets the conversation's goal, anew.
function goal.set(state, condition, continuations, budget)
	state.goal = {
		condition = condition,
		max_continuations = continuations,
		budget = budget,
		status = "active",
		continuations = 0,
		spent = 0,
		checks = {},
	}
end

-- Clears it.
function goal.clear(state)
	state.goal = nil
end

-- A check of the goal: its `n`, whether it was `met`, Jev's `p`, the
-- `turn` the model stopped at, the `continuation` it sent the model
-- back for, if it did, what it `cost` and what the goal `spent` so far.
function goal.check(state, check)
	local g = state.goal
	g.spent = check.spent
	g.error = nil
	if check.continuation ~= nil then
		g.continuations = check.continuation
		state.held[tostring(check.continuation)] = true
	end
	if check.met then
		g.status = "met"
	end
	table.insert(g.checks, check)
	local p = string.format("%.2f", check.p)
	if check.met then
		note(state, "the goal is met", "check " .. check.n .. " · turn " .. check.turn .. " · p " .. p .. " · the run stops", "good")
		return
	end
	local next = check.continuation and ("continuing " .. check.continuation .. " of " .. g.max_continuations)
		or "not continuing"
	note(state, "the goal is not met yet", "check " .. check.n .. " · turn " .. check.turn .. " · p " .. p .. " · " .. next, "warn")
end

-- Not met, and out of continuations or budget (`why`).
function goal.stopped(state, why)
	state.goal.status = "stopped"
	state.goal.exhausted = why
	local said = why == "budget" and "out of budget" or "out of continuations"
	note(state, "stopped the goal: " .. said .. ", and it is not met", "before_stop", "bad")
end

-- More continuations, and the goal active again.
function goal.extend(state, by)
	local g = state.goal
	g.max_continuations = math.max(g.continuations, g.max_continuations) + by
	g.status = "active"
	g.exhausted = nil
end

-- Kept, but not checked, until resumed.
function goal.pause(state)
	if state.goal and state.goal.status == "active" then
		state.goal.status = "paused"
	end
end

function goal.resume(state)
	if state.goal and state.goal.status == "paused" then
		state.goal.status = "active"
	end
end

-- Jev gave no answer; the run stopped unchecked.
function goal.failed(state, message)
	state.goal.error = message
	note(state, message, "not checked", "bad")
end

-- Where the goal stands, in a line.
function goal.status_line(g)
	local used = g.continuations .. " of " .. g.max_continuations .. " continuations"
	local spent = goal.usd(g.spent) .. " of " .. goal.usd(g.budget)
	local checks = #g.checks
	local line
	if g.status == "active" and checks == 0 then
		line = "set · checked when the agent stops"
	elseif g.status == "active" then
		line = "check " .. checks .. " not met · " .. used .. " · " .. spent
	elseif g.status == "paused" then
		line = "paused · " .. used .. " · " .. spent
	elseif g.status == "met" then
		line = "met at check " .. checks .. " · " .. g.continuations .. " continuations · " .. goal.usd(g.spent)
	elseif g.exhausted == "budget" then
		line = "stopped: over its " .. goal.usd(g.budget) .. " budget · not met"
	else
		line = "stopped: "
			.. g.continuations
			.. " of "
			.. g.max_continuations
			.. " continuations used · not met · "
			.. goal.usd(g.spent)
	end
	if g.error ~= nil then
		return line .. " · the last check failed"
	end
	return line
end

-- The plugin's line in the run's plugin list.
function goal.status(g)
	if g.status == "active" then
		return g.continuations .. " of " .. g.max_continuations .. " continuations"
	elseif g.status == "paused" then
		return "paused"
	elseif g.status == "met" then
		return "met at check " .. #g.checks
	end
	return "stopped, not met"
end

-- The goal as the sidebar says it: `2/10`, `met`.
function goal.badge(g)
	if g.status == "paused" then
		return "paused"
	elseif g.status == "met" then
		return "met"
	end
	return g.continuations .. "/" .. g.max_continuations
end

-- The goal's tone: working, met, or stopped.
function goal.tone(g)
	if g.status == "met" then
		return "good"
	elseif g.status == "stopped" then
		return "bad"
	end
	return "warn"
end

-- Why nothing checks a goal still to be met, if nothing does: the run
-- it was last kept in started without Jev.
function goal.unchecked(g, checks)
	if g.status ~= "active" and g.status ~= "paused" then
		return nil
	end
	if not checks then
		return "Not checked: tau-goal needs a TypeSafe key (Models)."
	end
	return nil
end

-- What the model reads when a check sends it back.
function goal.continuation(g, n, p, continuation)
	return goal.CONTINUATION_PREFIX
		.. "the goal is not met yet (check "
		.. n
		.. ", probability "
		.. string.format("%.2f", p)
		.. ").\nGoal: "
		.. g.condition
		.. "\n\nKeep working toward it, and run what proves it. Continuation "
		.. continuation
		.. " of "
		.. g.max_continuations
		.. "."
end

-- The limits the `/goal` popover's drafts say, or the defaults where one
-- does not read as a number.
function goal.limits(continuations, budget)
	local n = continuations_of(trim(continuations or ""))
	local usd = budget_of(trim(budget or ""))
	return n or goal.DEFAULT_CONTINUATIONS, usd or goal.DEFAULT_BUDGET
end

return goal
