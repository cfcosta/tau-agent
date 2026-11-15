-- What heiko-goal knows of goals (docs/reference/goal.md): how `/goal`
-- reads, the input the model gets for a goal, and a conversation's goal
-- as its checks leave it, with the notes, lines and badges that say it.
local heiko = require("heiko")

local goal = {}

-- Continuations a goal allows when `/goal` does not say.
goal.DEFAULT_CONTINUATIONS = 10

-- Tokens a goal may use when `/goal` does not say: the run's turns,
-- from when it was set.
goal.DEFAULT_BUDGET = 2000000

-- The probability at or past which the goal counts as met.
goal.MET_AT = 0.7

-- How many of the latest tool results Jev sees.
goal.EVIDENCE = 6

-- How much of a tool result Jev sees: its end, where results and
-- summaries are.
goal.OUTPUT_CHARS = 1500

-- How the model is told about its goal, after the condition.
goal.INSTRUCTIONS = "This is your goal. Keep working until it holds. Each time you stop, heiko-goal checks it against your last answer and your recent tool results, and sends you back if it does not hold yet, so run what proves it before you stop."

-- How a continuation starts, so interfaces can tell it from a person's
-- message.
goal.CONTINUATION_PREFIX = "heiko-goal: "

-- What a person sends to go on toward the goal.
goal.KEEP_GOING = "Keep going toward the goal."

-- What the model reads when a message clears the goal.
goal.CLEARED = "The goal is cleared."

-- `text` from its first character that is not white space.
local function trim_start(text)
	for at, c in utf8.codes(text) do
		if not heiko.text.is_space(c) then
			return string.sub(text, at)
		end
	end
	return ""
end

-- The first word of `text`, up to white space, and what follows it, its
-- leading white space gone.
local function next_word(text)
	for at, c in utf8.codes(text) do
		if heiko.text.is_space(c) then
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
		return heiko.text.is_space(c)
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

-- What `k` and `m` after a budget's digits multiply them by.
local SHORT = { [""] = 1, k = 1000, m = 1000000 }

-- The most tokens a budget says: past it a number is no longer whole.
local MOST_TOKENS = 2 ^ 53

-- A budget: a whole number of tokens, as digits or short, `500k` or
-- `2m`, in either case.
local function budget_of(text)
	local digits, unit = string.match(text, "^(%d+)([kKmM]?)$")
	if digits == nil then
		return nil
	end
	local tokens = tonumber(digits) * SHORT[string.lower(unit)]
	if tokens > MOST_TOKENS then
		return nil
	end
	return tokens
end

-- A count of tokens as a budget is typed, the shortest that reads back
-- as the same count: `500k`, `2m`, `1234`.
function goal.tokens(count)
	if count > 0 and count % 1000000 == 0 then
		return string.format("%.0fm", count / 1000000)
	elseif count > 0 and count % 1000 == 0 then
		return string.format("%.0fk", count / 1000)
	end
	return string.format("%.0f", count)
end

-- Reads `/goal [--continuations N] [--budget TOKENS] <condition>` or
-- `/goal clear`: `{ kind = "set", condition, continuations, budget }`,
-- `{ kind = "clear" }`, or nil for anything else, a budget that is not
-- a whole number of tokens included.
--
-- A condition is its words joined by single spaces, unless it is one
-- quoted string, `"..."`: that is read exactly as written, with `\"`,
-- `\\`, `\n` and `\r` for a quote, a backslash, a line feed and a
-- carriage return. `/goal "clear"` sets the goal `clear`.
function goal.parse(input)
	local rest = heiko.text.trim(input)
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
			local tokens
			tokens, rest = next_word(after)
			budget = budget_of(tokens)
			if budget == nil then
				return nil
			end
		else
			break
		end
	end
	local quoted = unquote(rest)
	if quoted ~= nil then
		if heiko.text.trim(quoted) == "" then
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

-- The plugin's state, as each hook finds it: `goal`, the conversation's
-- (`condition`, `max_continuations`, `budget`, `status` one of
-- "active", "paused", "met" and "stopped", `exhausted` when stopped,
-- `continuations` used, tokens `spent`, `checks` oldest first, `error`); its
-- `notes` in the transcript by key; `held`, the continuations a check's
-- note already says; and `said`, how the person's messages it knows
-- draw, by their text.
function goal.state(state)
	state.notes = state.notes or {}
	state.held = state.held or {}
	state.said = state.said or {}
	return state
end

-- Puts a note in the transcript: what happened (`kind`: "met",
-- "not_met", "stopped" or "failed"), and whether each check so far was
-- met, oldest first.
local function note(state, text, detail, tone, kind)
	local checks = array({})
	for _, check in ipairs(state.goal and state.goal.checks or {}) do
		table.insert(checks, check.met == true)
	end
	state.notes["g" .. heiko.values.count(state.notes)] = { text = text, detail = detail, tone = tone, kind = kind, checks = checks }
end

-- The fields every goal carries, whatever its status.
local KEPT = { "condition", "max_continuations", "budget", "continuations", "spent", "checks" }

local function fields(extra)
	local out = table.clone(KEPT)
	for _, name in ipairs(extra or {}) do
		table.insert(out, name)
	end
	return out
end

-- `g`'s kept fields, as `status`, with `extra`.
local function as(g, status, extra)
	local out = { status = status }
	for _, name in ipairs(KEPT) do
		out[name] = g[name]
	end
	for key, value in pairs(extra or {}) do
		out[key] = value
	end
	return out
end

local function active(g)
	return as(g, "active")
end

-- A goal's lifecycle (ADR 0054), its status the tag: active, paused, met,
-- or stopped when out of continuations or budget. Only an active goal is
-- checked, paused, stopped or failed; a failed check's `error` is an
-- active goal's, and `exhausted` a stopped one's.
goal.machine = heiko.machine({
	tag = "status",
	states = {
		active = fields({ "error?" }),
		paused = fields(),
		met = fields(),
		stopped = fields({ "exhausted" }),
	},
	initial = {
		status = "active",
		condition = "",
		max_continuations = goal.DEFAULT_CONTINUATIONS,
		budget = goal.DEFAULT_BUDGET,
		continuations = 0,
		spent = 0,
		checks = {},
	},
	on = {
		check = {
			active = function(g, e)
				-- A copy by hand: `table.clone` refuses the metatable of a
				-- list read back from JSON.
				local checks = {}
				for _, check in ipairs(g.checks) do
					table.insert(checks, check)
				end
				table.insert(checks, e.check)
				return as(g, e.check.met and "met" or "active", {
					spent = e.check.spent,
					continuations = e.check.continuation or g.continuations,
					checks = checks,
				})
			end,
		},
		stop = {
			active = function(g, e)
				return as(g, "stopped", { exhausted = e.why })
			end,
		},
		extend = {
			active = function(g, e)
				return as(g, "active", { max_continuations = math.max(g.continuations, g.max_continuations) + e.by })
			end,
		},
		pause = { active = function(g)
			return as(g, "paused")
		end },
		resume = { paused = active },
		fail = {
			active = function(g, e)
				return as(g, "active", { error = e.message })
			end,
		},
	},
})
for _, from in ipairs({ "paused", "met", "stopped" }) do
	goal.machine.on.extend[from] = goal.machine.on.extend.active
end

-- Sets the conversation's goal, anew: the tokens it spends count from
-- now.
function goal.set(state, condition, continuations, budget)
	state.unchecked = 0
	state.goal = {
		condition = condition,
		max_continuations = continuations,
		budget = budget,
		status = "active",
		continuations = 0,
		spent = 0,
		checks = {},
	}
	assert(goal.machine:shape(state.goal))
end

-- Clears it.
function goal.clear(state)
	state.goal = nil
end

-- A check of the goal: its `n`, whether it was `met`, Jev's `p`, the
-- `turn` the model stopped at, the `continuation` it sent the model
-- back for, if it did, and the tokens the goal `spent` so far.
function goal.check(state, check)
	state.goal = goal.machine:step(state.goal, { event = "check", check = check })
	local g = state.goal
	if check.continuation ~= nil then
		state.held[tostring(check.continuation)] = true
	end
	local p = string.format("%.2f", check.p)
	if check.met then
		note(state, "the goal is met", "check " .. check.n .. " · turn " .. check.turn .. " · p " .. p .. " · the run stops", "good", "met")
		return
	end
	local next = check.continuation and ("continuing " .. check.continuation .. " of " .. g.max_continuations)
		or "not continuing"
	note(state, "the goal is not met yet", "check " .. check.n .. " · turn " .. check.turn .. " · p " .. p .. " · " .. next, "warn", "not_met")
end

-- Not met, and out of continuations or budget (`why`).
function goal.stopped(state, why)
	state.goal = goal.machine:step(state.goal, { event = "stop", why = why })
	local said = why == "budget" and "out of budget" or "out of continuations"
	note(state, "stopped the goal: " .. said .. ", and it is not met", "before_stop", "bad", "stopped")
end

-- More continuations, and the goal active again.
function goal.extend(state, by)
	state.goal = goal.machine:step(state.goal, { event = "extend", by = by })
end

-- Kept, but not checked, until resumed.
function goal.pause(state)
	if state.goal then
		state.goal = goal.machine:step(state.goal, { event = "pause" })
	end
end

function goal.resume(state)
	if state.goal then
		state.goal = goal.machine:step(state.goal, { event = "resume" })
	end
end

-- Jev gave no answer; the run stopped unchecked.
function goal.failed(state, message)
	state.goal = goal.machine:step(state.goal, { event = "fail", message = message })
	note(state, message, "not checked", "bad", "failed")
end

-- Where the goal stands, in a line.
function goal.status_line(g)
	local used = g.continuations .. " of " .. g.max_continuations .. " continuations"
	local checks = #g.checks
	local line
	if g.status == "active" and checks == 0 then
		line = "set · checked when the agent stops"
	elseif g.status == "active" then
		line = "check " .. checks .. " not met · " .. used
	elseif g.status == "paused" then
		line = "paused · " .. used
	elseif g.status == "met" then
		line = "met at check " .. checks .. " · " .. g.continuations .. " continuations"
	elseif g.exhausted == "budget" then
		line = "stopped: over its budget · not met"
	else
		line = "stopped: " .. used .. " used · not met"
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
		return "Not checked: heiko-goal needs a TypeSafe key (Models)."
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
	local n = continuations_of(heiko.text.trim(continuations or ""))
	local tokens = budget_of(heiko.text.trim(budget or ""))
	return n or goal.DEFAULT_CONTINUATIONS, tokens or goal.DEFAULT_BUDGET
end

return goal
