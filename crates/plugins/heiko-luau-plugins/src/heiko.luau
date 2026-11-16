-- The `heiko` module every Luau plugin and agent requires (ADR 0027,
-- ADR 0037): a plugin's or an agent's declaration, the answers a
-- plugin's hooks give, the view pieces, and what the host uses to read
-- them and call their hooks.

local heiko = {}
local ui = {}
heiko.ui = ui

-- A plugin: its table, checked.
function heiko.plugin(spec)
	if type(spec) ~= "table" then
		error("heiko.plugin takes a table", 2)
	end
	if spec.name ~= nil or spec.description ~= nil then
		error("a plugin's name and description are in its heiko.toml", 2)
	end
	-- Commands from what the plugin keeps of the app (ADR 0050, gap K):
	-- `list(app)` gives them by name, and `run(name, text, ctx)` carries
	-- one out, as the action `/name`.
	if type(spec.commands) == "table" and type(spec.commands.list) == "function" then
		local listed = spec.commands
		if type(listed.run) ~= "function" then
			error("listed commands need a `run`", 2)
		end
		spec.listed = listed
		-- What the host lists of them, as `heiko._commands` reads a table.
		listed._list = function(input)
			return heiko._commands(listed.list(input))
		end
		spec.commands = nil
		spec.actions = setmetatable(spec.actions or {}, {
			__index = function(_, key)
				if type(key) == "string" and string.sub(key, 1, 1) == "/" then
					return function(args, model, env)
						env.drafts = args.drafts or {}
						return listed.run(string.sub(key, 2), args.text or "", model, env)
					end
				end
				return nil
			end,
		})
	end
	-- Each command runs as the action `/name`, given the text after it.
	for name, command in pairs(spec.commands or {}) do
		if type(command) ~= "table" or type(command.run) ~= "function" then
			error("command `" .. tostring(name) .. "` needs a `run`", 2)
		end
		spec.actions = spec.actions or {}
		spec.actions["/" .. name] = function(args, model, env)
			-- What the popover's inputs say, by id.
			env.drafts = args.drafts or {}
			return command.run(args.text or "", model, env)
		end
	end
	-- What `infer{ tools }` calls the plugin's own tools from.
	heiko._spec = spec
	return spec
end

-- An agent (ADR 0037): its table, checked. What wakes it is its
-- `triggers`, each by the name its events carry.
function heiko.agent(spec)
	if type(spec) ~= "table" then
		error("heiko.agent takes a table", 2)
	end
	if spec.name ~= nil or spec.description ~= nil then
		error("an agent's name and description are in its heiko.toml", 2)
	end
	if spec.triggers ~= nil and type(spec.triggers) ~= "table" then
		error("an agent's `triggers` is a table of triggers by name", 2)
	end
	return spec
end

-- A secret by name, from heiko's secret store: what an HTTP request's
-- headers or body may hold. The host puts the value in as the request
-- leaves; Luau never sees it. `"Bearer " .. heiko.secret("github")` is the
-- secret with that text before its value, as is
-- `heiko.secret("github", "Bearer ")`.
local Secret = {}

function heiko.secret(name, prefix)
	if type(name) ~= "string" or name == "" then
		error("heiko.secret takes a secret's name", 2)
	end
	if prefix ~= nil and type(prefix) ~= "string" then
		error("heiko.secret's prefix is text", 2)
	end
	return setmetatable({ ["$secret"] = name, prefix = prefix }, Secret)
end

Secret.__concat = function(left, right)
	if type(left) == "string" and getmetatable(right) == Secret then
		return heiko.secret(right["$secret"], left .. (right.prefix or ""))
	end
	error('a secret can only follow text, as in "Bearer " .. heiko.secret("github")', 2)
end

-- A plugin's commands as the host reads them: each by name, with what it
-- does and what follows it, in name order.
function heiko._commands(commands)
	local list = array({})
	for name, command in pairs(commands or {}) do
		table.insert(list, {
			name = name,
			description = command.description or "",
			args = command.args or "",
			icon = command.icon or "",
			popover = command.popover and {
				decide = command.popover.decide,
				no_decide = command.popover.no_decide or command.popover.decide,
			} or json.null,
		})
	end
	table.sort(list, function(a, b)
		return a.name < b.name
	end)
	return list
end

-- How many times a plugin's `before_stop` may keep a run going, as it
-- declares: a whole number from 1 to 20, or `json.null` for heiko's own.
function heiko._continues(continues)
	if continues == nil then
		return json.null
	end
	if type(continues) ~= "number" or continues % 1 ~= 0 or continues < 1 or continues > 20 then
		error("`continues` is a whole number from 1 to 20", 0)
	end
	return continues
end

-- The names of a table's functions, in order.
function heiko._names(functions)
	local names = array({})
	for name in pairs(functions or {}) do
		table.insert(names, name)
	end
	table.sort(names)
	return names
end

-- A plugin's jobs as the host reads them: each by name, with how often
-- it runs and what it does, in name order.
function heiko._jobs(jobs)
	local list = array({})
	for name, job in pairs(jobs or {}) do
		if type(job) ~= "table" or type(job.run) ~= "function" then
			error("job `" .. tostring(name) .. "` needs a `run`", 2)
		end
		table.insert(list, {
			name = name,
			every = math.floor(tonumber(job.every) or 0),
			label = job.label or name,
		})
	end
	table.sort(list, function(a, b)
		return a.name < b.name
	end)
	return list
end

-- A `forge` table as the host reads it: what it names, and which of its
-- hooks it has.
function heiko._forge(forge)
	local hooks = array({})
	for _, name in ipairs({ "account", "repos", "can_push", "open_pull", "checks", "refused" }) do
		if type(forge[name]) == "function" then
			table.insert(hooks, name)
		end
	end
	return {
		name = forge.name or "",
		id = forge.id or "",
		host = forge.host or "",
		token = forge.token or "",
		hooks = hooks,
	}
end

-- What an action answers to start a chat in `repo` (the action's own
-- when it names none) on the person's `text`, once the action ended and
-- its store is written: then `actions[then]` is called with `args` and
-- the chat's `run` and `repo`, or the `error` that kept it from
-- starting.
function heiko.start_chat(chat)
	return {
		heiko_chat = {
			start = { repo = chat.repo, text = tostring(chat.text) },
			["then"] = chat["then"],
			args = chat.args or {},
		},
	}
end

-- What an action answers to have the interface do one of heiko's button
-- actions once it is done, on the action's run: `composer` (`text`),
-- `send` (`text`), `steer` (`text`), `start` (`repo`, `text`), `alert`
-- (`title`, `message`) or `details`.
function heiko.interface(action, args)
	return { heiko_interface = { action = tostring(action), args = args or {} } }
end

-- What an action answers to say the person's `text` to the chat `run`,
-- steering a turn going or starting one; `then` and `args` as
-- `heiko.start_chat`'s.
function heiko.steer_chat(chat)
	return {
		heiko_chat = {
			steer = { run = tostring(chat.run), text = tostring(chat.text) },
			["then"] = chat["then"],
			args = chat.args or {},
		},
	}
end

-- What a tool answers to ask the person first: a card in the run, and
-- on phones, says `title` and `text`. Allowed, `actions[action]` is
-- carried out with `args` as the person's own, and the model reads what
-- it answered; denied, the model reads why.
function heiko.allow(ask)
	return {
		heiko_allow = {
			title = tostring(ask.title),
			text = ask.text and tostring(ask.text) or "",
			action = tostring(ask.action),
			args = ask.args or {},
		},
	}
end

-- What a tool answers to ask the person a form and wait for the
-- answers, from whichever device answers first: one to four
-- `questions`, each `{ question, header?, options = { { label,
-- description?, preview? } }, multi_select?, other? }`, `other` letting
-- the person write their own answer. The plugin's view draws the form
-- (`composer`); one of its actions gives the answers (`heiko.answer`).
-- Then `actions[action]` is called in the run with `{ call, answers }`
-- or `{ call, declined = true }`, and the model reads what it answers.
function heiko.ask(form)
	return {
		heiko_ask = {
			questions = form.questions or array({}),
			action = form.action and tostring(form.action) or json.null,
		},
	}
end

-- What an action answers to give the call `call`, waiting on
-- `heiko.ask`, the person's `answers` (each `{ picked = { labels },
-- other?, note? }`, in the questions' order), or with `declined = true`
-- that they would not answer. The first answer takes it; one that does
-- not fit the form, or comes once it is taken, is the action's error.
function heiko.answer(answer)
	return {
		heiko_answer = {
			call = tostring(answer.call),
			answers = answer.answers or json.null,
			declined = answer.declined == true,
		},
	}
end

-- What `before_tool` answers.
function heiko.block(reason)
	return { decision = "block", reason = tostring(reason) }
end

-- What `start` answers: `context`, texts the run's first message
-- starts with, `instructions`, added to the run's own, and `effort`, the
-- reasoning effort the run starts at when it is on auto.
function heiko.start(plan)
	return {
		context = plan.context or array({}),
		instructions = plan.instructions or json.null,
		effort = plan.effort or json.null,
		input = plan.input or json.null,
	}
end

-- What `start` answers to end the run at once, with `why` as its error:
-- what the plugin needs to check the run is broken, and it says so
-- rather than let the run go on unchecked.
function heiko.fail(why)
	return { fail = tostring(why) }
end

-- What `rewrite_context` answers to rewrite the context: `keep`, the
-- messages to keep by their `index`; `insert`, texts to put `before` a
-- message; `drops_conversation`, whether it drops what was said.
function heiko.rewrite(rewrite)
	return {
		rewrite = {
			keep = rewrite.keep or array({}),
			insert = rewrite.insert or array({}),
			drops_conversation = rewrite.drops_conversation == true,
			details = rewrite.details or json.null,
			-- Calls dropped, by id, with their results; and messages
			-- whose text is replaced, each `{ index, text }`.
			drop_calls = rewrite.drop_calls or json.null,
			texts = rewrite.texts or json.null,
		},
	}
end

-- What `after_tool_result` answers to have the model read `text` in
-- place of the call's result.
-- With `{ flag = false }` the call is not marked for the person's
-- review.
function heiko.result(text, options)
	return { text = tostring(text), flag = not (options and options.flag == false) }
end

-- What `before_request` answers to set the turn's reasoning effort:
-- "low", "medium", "high" and so on.
function heiko.effort(level)
	return { effort = tostring(level) }
end

-- What `before_stop` answers to go on.
function heiko.continue(message)
	return { decision = "continue", message = tostring(message) }
end

-- State machines (ADR 0054): a machine holds no state. A state is a
-- plain table, `{ state = "<tag>", ...fields }`, kept where the plugin
-- keeps state, so it persists and replicates; `step` moves it.
local Machine = {}
Machine.__index = Machine

-- `fields` of a tag as a set: `name` required, `name?` optional. `key`
-- is the field that names the tag.
local function field_set(tag, fields, key)
	if type(fields) ~= "table" then
		error("heiko.machine: the state `" .. tostring(tag) .. "` needs a list of fields", 3)
	end
	local set = {}
	for _, field in ipairs(fields) do
		local name, optional = tostring(field), false
		if string.sub(name, -1) == "?" then
			name, optional = string.sub(name, 1, -2), true
		end
		if name == key then
			error("heiko.machine: `" .. key .. "` names the tag, not a field of `" .. tag .. "`", 3)
		end
		set[name] = optional and "optional" or "required"
	end
	return set
end

-- A machine: `states` (each tag's fields), `initial` (a state), `on`
-- (by event, then by the tag it leaves: a function of the state and the
-- event giving the next state and its effects), and optionally `effects`
-- (the names effects may have), `strict` (an event with no arm is an
-- error, not a step that changes nothing) and `tag` (the field that names
-- a state's tag, `state` unless said: data kept before can keep its own).
function heiko.machine(spec)
	if type(spec) ~= "table" or type(spec.states) ~= "table" or type(spec.on) ~= "table" then
		error("heiko.machine needs `states` and `on`", 2)
	end
	local machine = setmetatable({
		fields = {},
		on = {},
		strict = spec.strict == true,
		tag = spec.tag and tostring(spec.tag) or "state",
	}, Machine)
	for tag, fields in pairs(spec.states) do
		machine.fields[tostring(tag)] = field_set(tostring(tag), fields, machine.tag)
	end
	for event, arms in pairs(spec.on) do
		if type(arms) ~= "table" then
			error("heiko.machine: the event `" .. tostring(event) .. "` needs its arms, by state", 2)
		end
		for tag, arm in pairs(arms) do
			if machine.fields[tag] == nil then
				error("heiko.machine: `" .. event .. "` leaves `" .. tostring(tag) .. "`, which is no state", 2)
			end
			if type(arm) ~= "function" then
				error("heiko.machine: `" .. event .. "` from `" .. tag .. "` is no function", 2)
			end
		end
		machine.on[tostring(event)] = arms
	end
	if spec.effects ~= nil then
		machine.effects = {}
		for _, name in ipairs(spec.effects) do
			machine.effects[tostring(name)] = true
		end
	end
	local ok, why = machine:shape(spec.initial)
	if not ok then
		error("heiko.machine: `initial` " .. why, 2)
	end
	machine.initial = spec.initial
	return machine
end

-- Whether `state` is in shape: a known tag, and exactly its fields.
-- Returns false and why when it is not.
function Machine:shape(state)
	if type(state) ~= "table" then
		return false, "is " .. type(state) .. ", not a state"
	end
	local tag = state[self.tag]
	local fields = self.fields[tag]
	if fields == nil then
		return false, "names no state: `" .. tostring(tag) .. "`"
	end
	for key in pairs(state) do
		if key ~= self.tag and fields[key] == nil then
			return false, "of `" .. tag .. "` has `" .. tostring(key) .. "`, which it does not carry"
		end
	end
	for name, kind in pairs(fields) do
		if kind == "required" and state[name] == nil then
			return false, "of `" .. tag .. "` lacks `" .. name .. "`"
		end
	end
	return true
end

-- Whether `state` is the tag `tag`.
function Machine:is(state, tag)
	return type(state) == "table" and state[self.tag] == tag
end

-- The states' tags, sorted.
function Machine:tags()
	local tags = {}
	for tag in pairs(self.fields) do
		table.insert(tags, tag)
	end
	table.sort(tags)
	return tags
end

-- The events, sorted.
function Machine:events()
	local events = {}
	for event in pairs(self.on) do
		table.insert(events, event)
	end
	table.sort(events)
	return events
end

-- Whether `event` has an arm from `state`'s tag.
function Machine:handles(state, event)
	local arms = self.on[event]
	return arms ~= nil and type(state) == "table" and arms[state[self.tag]] ~= nil
end

local function copy(value)
	if type(value) ~= "table" then
		return value
	end
	local out = {}
	for key, item in pairs(value) do
		out[key] = copy(item)
	end
	return out
end

-- The state after `event` (a table whose `event` names it) from
-- `state` (nil: `initial`), and the effects to carry out. An event with
-- no arm from the state changes nothing, unless the machine is strict.
function Machine:step(state, event)
	if state == nil then
		state = copy(self.initial)
	end
	local ok, why = self:shape(state)
	if not ok then
		error("heiko.machine: the state " .. why, 2)
	end
	if type(event) ~= "table" or type(event.event) ~= "string" then
		error("heiko.machine: an event is a table whose `event` names it", 2)
	end
	local from = state[self.tag]
	local arms = self.on[event.event]
	local arm = arms and arms[from]
	if arm == nil then
		if self.strict then
			error("heiko.machine: `" .. event.event .. "` has no arm from `" .. from .. "`", 2)
		end
		return state, {}
	end
	local next, effects = arm(state, event)
	ok, why = self:shape(next)
	if not ok then
		error("heiko.machine: `" .. event.event .. "` from `" .. from .. "` gave a state that " .. why, 2)
	end
	effects = effects or {}
	if type(effects) ~= "table" then
		error("heiko.machine: `" .. event.event .. "` from `" .. from .. "` gave effects that are no list", 2)
	end
	for _, effect in ipairs(effects) do
		local name = type(effect) == "table" and effect[1] or effect
		if type(name) ~= "string" then
			error("heiko.machine: an effect of `" .. event.event .. "` is named by no string", 2)
		end
		if self.effects ~= nil and not self.effects[name] then
			error("heiko.machine: `" .. event.event .. "` gave the effect `" .. name .. "`, which the machine does not declare", 2)
		end
	end
	return next, effects
end

-- The view pieces: plain tables a Rust renderer draws.
local TONES = { neutral = true, good = true, warn = true, bad = true, info = true }

local function tone(value)
	if value == nil then
		return "neutral"
	end
	if not TONES[value] then
		error("a tone is neutral, good, warn, bad or info, not " .. tostring(value), 3)
	end
	return value
end

local function pieces(children)
	local out = array({})
	for _, child in ipairs(children or {}) do
		if type(child) == "string" then
			child = ui.text(child)
		end
		table.insert(out, child)
	end
	return out
end

function ui.text(text)
	return { piece = "text", text = tostring(text) }
end

-- Prose with `code` and **bold**.
function ui.rich(text)
	return { piece = "rich", text = tostring(text) }
end

function ui.mono(text)
	return { piece = "mono", text = tostring(text) }
end

function ui.badge(text, of)
	return { piece = "badge", text = tostring(text), tone = tone(of) }
end

-- Rows of a key and a value: a list of `{ key, value }`, or a table,
-- in its keys' order.
function ui.rows(rows)
	local out = array({})
	if rows[1] ~= nil then
		for _, row in ipairs(rows) do
			table.insert(out, { key = tostring(row[1]), value = tostring(row[2]) })
		end
	else
		local keys = {}
		for key in pairs(rows) do
			table.insert(keys, tostring(key))
		end
		table.sort(keys)
		for _, key in ipairs(keys) do
			table.insert(out, { key = key, value = tostring(rows[key]) })
		end
	end
	return { piece = "rows", rows = out }
end

function ui.list(items)
	return { piece = "list", items = pieces(items) }
end

function ui.progress(share, label)
	share = math.max(0, math.min(1, tonumber(share) or 0))
	return { piece = "progress", share = share, label = label and tostring(label) or json.null }
end

-- Code, highlighted as `lang`; `opts.first_line` numbers its lines.
function ui.code(lang, text, opts)
	opts = opts or {}
	return {
		piece = "code",
		lang = lang and tostring(lang) or json.null,
		text = tostring(text),
		first_line = opts.first_line and math.floor(tonumber(opts.first_line) or 1) or json.null,
	}
end

-- A line in a tone: `good` for a passed test, `neutral` dims it.
-- `mono` sets it in the monospace face.
-- A big line, as a screen's headline, with a line under it.
function ui.toned(text, of, mono)
	return { piece = "toned", text = tostring(text), tone = tone(of), mono = mono == true }
end

function ui.stack(children)
	return { piece = "stack", children = pieces(children) }
end

-- Pieces on one line that never wraps: the last takes what is left of
-- the width, cut where it does not fit.
function ui.line(children)
	return { piece = "line", children = pieces(children) }
end

function ui.row(children)
	return { piece = "row", children = pieces(children) }
end

local KINDS = { primary = true, secondary = true, danger = true, hot = true }
local SIZES = { small = true, medium = true, large = true }

-- A button that carries out the plugin's `actions[action]` with `args`;
-- `kind` is primary, secondary (the default), danger or hot: one that
-- needs the person. `icon`, one of heiko's icons by name, goes before the
-- label, or alone with an empty one. `size` is a primary button's:
-- small (the default), medium or large. A `wide` one takes the whole
-- width, its label at the left; a wide primary one may have `detail`,
-- a second line in smaller text inside the block.
function ui.button(label, action, args, kind, icon, size, wide, detail)
	kind = kind or "secondary"
	if not KINDS[kind] then
		error("a button's kind is primary, secondary, danger or hot, not " .. tostring(kind), 2)
	end
	if size ~= nil and not SIZES[size] then
		error("a button's size is small, medium or large, not " .. tostring(size), 2)
	end
	return {
		piece = "button",
		label = tostring(label),
		action = tostring(action),
		args = args or {},
		kind = kind,
		icon = icon and tostring(icon) or nil,
		size = size,
		wide = wide == true or nil,
		detail = detail ~= nil and tostring(detail) or nil,
	}
end

local GAPS = { small = true, medium = true, large = true }

-- Room between two pieces one under the other: small, medium (the
-- default) or large. A header after other pieces has room over it
-- already.
function ui.gap(size)
	size = size or "medium"
	if not GAPS[size] then
		error("a gap is small, medium or large, not " .. tostring(size), 2)
	end
	return { piece = "gap", size = size }
end

-- A labelled choice: `choices` side by side, each a string or
-- `{ label, value }`, the one whose value is `picked` outlined. A click
-- carries out the plugin's `actions[action]` with `args` and the
-- choice's `value`; a string's value is the string.
function ui.segments(choices, picked, action, args)
	local out = array({})
	for _, choice in ipairs(choices or {}) do
		if type(choice) == "table" then
			if choice.label == nil then
				error("a choice is a string or { label, value }", 2)
			end
			local value = choice.value
			if value == nil then
				value = tostring(choice.label)
			end
			table.insert(out, { label = tostring(choice.label), value = value })
		else
			table.insert(out, { label = tostring(choice), value = tostring(choice) })
		end
	end
	if picked == nil then
		picked = json.null
	end
	return { piece = "segments", choices = out, picked = picked, action = tostring(action), args = args or {} }
end

-- `text` as a QR code for a phone's camera, `opts.size` pixels wide
-- (200 when not given); with `opts.offset`, printed over the offset.
function ui.qr(text, opts)
	opts = opts or {}
	return {
		piece = "qr",
		text = tostring(text),
		size = math.max(40, tonumber(opts.size) or 200),
		offset = opts.offset == true or nil,
	}
end

-- A switch that shows `on`, and carries out the plugin's
-- `actions[action]` with `args` when flipped (ADR 0050, gap L);
-- `detail` is a line under its label.
function ui.switch(label, detail, on, action, args)
	return {
		piece = "switch",
		label = tostring(label),
		detail = detail and tostring(detail) or json.null,
		on = on == true,
		action = tostring(action),
		args = args or {},
	}
end

-- Markdown, drawn as a model's reply is.
function ui.markdown(text)
	return { piece = "markdown", text = tostring(text) }
end

-- One of heiko's icons, by name: `memory`, `sub-agent`, `check`.
function ui.icon(name, of)
	return { piece = "icon", name = tostring(name), tone = tone(of) }
end

function ui.pill(text, of)
	return { piece = "pill", text = tostring(text), tone = tone(of) }
end

-- The color a mark, a stamp, a square, a slip or a heading is printed
-- in: a tone, or one of the palette's other colors.
local INKS = {
	neutral = true,
	good = true,
	warn = true,
	bad = true,
	info = true,
	teal = true,
	cyan = true,
	purple = true,
	yellow = true,
	bright = true,
}

local INK_NAMES = "neutral, good, warn, bad, info, teal, cyan, purple, yellow or bright"

local function ink(value)
	if value == nil then
		return "neutral"
	end
	if not INKS[value] then
		error("a color is " .. INK_NAMES .. ", not " .. tostring(value), 3)
	end
	return value
end

-- A small label over a group, in the color `of` when given.
function ui.heading(text, of)
	return { piece = "heading", text = tostring(text), tone = of ~= nil and ink(of) or nil }
end

-- A title, in an ink when `of` names one.
function ui.title(text, subtitle, of)
	return {
		piece = "title",
		text = tostring(text),
		subtitle = subtitle and tostring(subtitle) or json.null,
		tone = of ~= nil and ink(of) or nil,
	}
end

-- Where a click goes: `opts.page` of this plugin, or of `opts.plugin`,
-- with `opts.params`; nothing without a page.
local function target(opts)
	if opts == nil or opts.page == nil then
		return nil
	end
	local params = {}
	for key, value in pairs(opts.params or {}) do
		params[tostring(key)] = tostring(value)
	end
	return { plugin = opts.plugin and tostring(opts.plugin) or json.null, page = tostring(opts.page), params = params }
end

-- A section's head, which does not fold: its title over a rule.
-- `opts.meta` is small print at its right; `opts.actions` are pieces
-- after it, such as buttons.
function ui.header(title, opts)
	opts = opts or {}
	return {
		piece = "header",
		title = tostring(title),
		meta = opts.meta and tostring(opts.meta) or json.null,
		actions = pieces(opts.actions),
	}
end

-- One line of code, parts of it marked: a line a search found, and
-- where. `spans` are `{ from, to }`, the first and last byte of each
-- part, from 1, as `string.find` gives them.
function ui.marked(text, spans)
	local out = array({})
	for _, span in ipairs(spans or {}) do
		local from = math.max(1, math.floor(tonumber(span[1]) or 1))
		local to = math.max(0, math.floor(tonumber(span[2]) or 0))
		table.insert(out, { from, to })
	end
	return { piece = "marked", text = tostring(text), spans = out }
end

-- Pieces one under the other on the darker plane: a result.
function ui.band(children)
	return { piece = "band", children = pieces(children) }
end

-- Pieces one under the other, a line down the left joining the squares
-- they start with: the steps of a change, a queue.
function ui.rail(children)
	return { piece = "rail", children = pieces(children) }
end

-- A quantity as `total` square ticks, the first `lit` in the color
-- `of`; `marked` are the places, from 0, of ticks drawn white, as
-- thresholds on the strip. `labels` are `{ at = place, text = text }`:
-- small print under the strip, each centred under the tick at its
-- place, from 0.
function ui.ticks(lit, total, of, marked, labels)
	local out = array({})
	for _, at in ipairs(marked or {}) do
		table.insert(out, math.max(0, math.floor(tonumber(at) or 0)))
	end
	local under = nil
	for _, label in ipairs(labels or {}) do
		under = under or array({})
		table.insert(under, { at = math.max(0, math.floor(tonumber(label.at) or 0)), text = tostring(label.text) })
	end
	total = math.max(1, math.floor(tonumber(total) or 1))
	return {
		piece = "ticks",
		lit = math.max(0, math.min(total, math.floor(tonumber(lit) or 0))),
		total = total,
		tone = ink(of),
		marked = out,
		labels = under,
	}
end

local STATES = { working = true, asks = true, failing = true, met = true, limit = true, cap = true, idle = true }

-- The small mark of a state: working, asks, failing, met, limit, cap or
-- idle. With a `count` above one, the count goes before it.
function ui.mark(state, count)
	if not STATES[state] then
		error("a mark is working, asks, failing, met, limit, cap or idle, not " .. tostring(state), 2)
	end
	return {
		piece = "mark",
		state = state,
		count = count and math.max(0, math.floor(tonumber(count) or 0)) or json.null,
	}
end

-- A word or two in mono, outlined in the color `of`: `exit 0`, `queued`.
function ui.stamp(label, of)
	return { piece = "stamp", label = tostring(label), tone = ink(of) }
end

local SQUARES = { quiet = true, outlined = true, filled = true }

-- A small square holding a number or a letter: a step, a check, a place
-- in a queue. `style` is quiet (the default: not reached yet), outlined
-- or filled, in the color `of`. With `action`, a click carries out the
-- plugin's `actions[action]` with `args`: a segment, a toggle.
function ui.square(label, style, of, action, args)
	style = style or "quiet"
	if not SQUARES[style] then
		error("a square is quiet, outlined or filled, not " .. tostring(style), 2)
	end
	return {
		piece = "square",
		label = tostring(label),
		style = style,
		tone = ink(of),
		action = action and tostring(action) or nil,
		args = action and (args or {}) or nil,
	}
end

-- What a tool does (ADR 0060), as its table's `kind` and a step say it.
local KINDS = { look = true, search = true, change = true, run = true, reach = true, people = true }

local STEP_STATES = { done = true, running = true, failed = true, waits = true, stopped = true }

-- A call made inside another (ADR 0060): a lettered step, as codemode
-- lists a script's calls. `step` holds its `label` (`a`) and `tool`, and
-- may hold its `kind` (look, search, change, run, reach or people), its
-- `subject` (a path, a command), its `receipt` (`29 lines`) and its
-- `state`: done (the default), running, failed, waits or stopped.
function ui.step(step)
	local state = step.state or "done"
	if not STEP_STATES[state] then
		error("a step is done, running, failed, waits or stopped, not " .. tostring(state), 2)
	end
	if step.kind ~= nil and not KINDS[step.kind] then
		error("a step's kind is look, search, change, run, reach or people, not " .. tostring(step.kind), 2)
	end
	return {
		piece = "step",
		label = tostring(step.label or ""),
		tool = tostring(step.tool or ""),
		kind = step.kind,
		subject = tostring(step.subject or ""),
		receipt = tostring(step.receipt or ""),
		state = state,
	}
end

-- A note or an id on a label outlined in the color `of`; `dashed` for
-- one proposed, not kept yet. With `opts.page`, a click opens that page
-- of this plugin, or of `opts.plugin`, with `opts.params`.
function ui.slip(text, of, dashed, opts)
	return { piece = "slip", text = tostring(text), tone = ink(of), dashed = dashed == true, to = target(opts) }
end

-- A big number or time, in the display face.
function ui.figure(text)
	return { piece = "figure", text = tostring(text) }
end

-- One small square for each ink in `inks`: a few things, by how each
-- went. `neutral` is one not there yet.
function ui.pips(inks)
	local out = array({})
	for _, of in ipairs(inks or {}) do
		table.insert(out, ink(of))
	end
	return { piece = "pips", tones = out }
end

-- A large glyph in the display face, in the color `of`: the mark beside
-- a note.
function ui.kanji(text, of)
	return { piece = "kanji", text = tostring(text), tone = ink(of) }
end

-- A header over rows; each row is a list of pieces or strings.
-- `opts.widths` are the columns' shares of the width, 1 each when not
-- given.
function ui.table(head, rows, opts)
	local h = array({})
	for _, cell in ipairs(head or {}) do
		table.insert(h, tostring(cell))
	end
	local out = array({})
	for _, row in ipairs(rows or {}) do
		table.insert(out, pieces(row))
	end
	local widths = nil
	if opts and opts.widths then
		widths = array({})
		for _, width in ipairs(opts.widths) do
			table.insert(widths, tonumber(width) or 1)
		end
	end
	return { piece = "table", head = h, rows = out, widths = widths }
end

-- Pieces that scroll past `height` rems (24 when not given).
function ui.scroll(children, height)
	return { piece = "scroll", children = pieces(children), height = height and tonumber(height) or json.null }
end

-- A heading that opens and closes `children`. `id` names it among the
-- plugin's sections, so where the person left it is kept; `opts.open`
-- says whether it starts open, and `opts.small` makes it a line inside a
-- note rather than a page's heading.
function ui.section(id, title, children, opts)
	return {
		piece = "section",
		id = tostring(id),
		title = tostring(title),
		open = (opts and opts.open) == true,
		small = (opts and opts.small) == true,
		lead = pieces(opts and opts.lead or {}),
		children = pieces(children),
	}
end

-- A text input. `id` names it, and heiko keeps what is typed in it, the
-- draft, on every device and across restarts. `opts.placeholder`,
-- `opts.multiline`, and `opts.submit`: the action Enter carries out,
-- with `{ text = draft }`.
function ui.input(id, opts)
	opts = opts or {}
	return {
		piece = "input",
		id = tostring(id),
		placeholder = opts.placeholder and tostring(opts.placeholder) or json.null,
		multiline = opts.multiline == true,
		submit = opts.submit and tostring(opts.submit) or json.null,
		keep = opts.keep == true,
		-- A new number gives it the keys, once.
		focus = opts.focus and math.floor(tonumber(opts.focus) or 0) or nil,
	}
end

-- An input for the secret `name`, one `uses.secrets` declares, on a page
-- of the app (ADR 0050, gap H): what is typed goes to heiko's secret store
-- on Enter, never to the plugin.
function ui.secret(name, opts)
	opts = opts or {}
	return {
		piece = "input",
		id = "secret/" .. tostring(name),
		placeholder = opts.placeholder and tostring(opts.placeholder) or json.null,
		multiline = false,
		submit = "heiko.secret/" .. tostring(name),
		keep = false,
		secret = true,
	}
end

-- An area that takes the keys: while it has them, each of `keys`
-- (`"up"`, `"enter"`, `"n"`, `"1"` ...) carries out `actions[action]` with
-- `{ key, field = false }`; in a field inside it, the keys of
-- `opts.through` still come, with `field = true`. A click outside its
-- fields gives it the keys, and so does a new `opts.focus` number.
function ui.keys(id, keys, action, children, opts)
	opts = opts or {}
	local names = array({})
	for _, key in ipairs(keys or {}) do
		table.insert(names, tostring(key))
	end
	local through = array({})
	for _, key in ipairs(opts.through or {}) do
		table.insert(through, tostring(key))
	end
	return {
		piece = "keys",
		id = tostring(id),
		keys = names,
		through = through,
		action = tostring(action),
		focus = opts.focus and math.floor(tonumber(opts.focus) or 0) or json.null,
		children = pieces(children),
	}
end

-- Pieces in a group of their own, under a hairline.
function ui.card(children)
	return { piece = "card", children = pieces(children) }
end

-- Room that pushes what follows in a row to its end.
function ui.spacer()
	return { piece = "spacer" }
end

-- A panel over the page, for a choice or a form: put it in the view only
-- while it is open. A click outside it carries out `opts.dismiss`.
function ui.modal(title, children, opts)
	opts = opts or {}
	return {
		piece = "modal",
		title = tostring(title),
		children = pieces(children),
		dismiss = opts.dismiss and tostring(opts.dismiss) or json.null,
	}
end

-- A unified diff, its lines numbered, colored as `path`'s language when
-- given.
function ui.diff(text, path)
	return { piece = "diff", text = tostring(text), path = path and tostring(path) or json.null }
end

-- What a program wrote to a terminal, escapes and all. `id` names it, so
-- heiko keeps its screen, scrolled where the person left it.
-- `opts.cols`, `opts.rows` (80 by 24 when not given), and `opts.live`
-- while more is coming.
function ui.terminal(id, text, opts)
	opts = opts or {}
	return {
		piece = "terminal",
		id = tostring(id),
		text = tostring(text),
		cols = tonumber(opts.cols) or 80,
		rows = tonumber(opts.rows) or 24,
		live = opts.live == true,
		fold = opts.fold == true,
	}
end

-- The time since `since`, in milliseconds since 1970 (`ctx.now.unix *
-- 1000` from a hook), counting until it `stopped`. `id` names it.
function ui.ticker(id, since, stopped)
	return {
		piece = "ticker",
		id = tostring(id),
		since = math.floor(tonumber(since) or 0),
		["until"] = stopped and math.floor(tonumber(stopped) or 0) or json.null,
	}
end

-- An image the plugin kept with `ctx.blobs.put_file`, in the repository
-- `repo` (`ctx.run.repo`): the interface asks for its bytes once and
-- keeps them. `height` is in rems; it keeps its shape.
function ui.image(blob, mime, repo, height)
	return {
		piece = "image",
		key = tostring(blob.id),
		mime = tostring(mime),
		fetch = { blob = blob, repo = tostring(repo) },
		height = height,
	}
end

-- The time left until `at`, in milliseconds since 1970 (`ctx.now.unix *
-- 1000` from a hook): `in 5m`, then `now`. `id` names it.
function ui.due(id, at)
	return { piece = "due", id = tostring(id), at = math.floor(tonumber(at) or 0) }
end

-- A small map: `nodes` are `{ id, label, x, y, tone, focus, page }`, with
-- `x` and `y` from 0 to 1 and `page` one of this plugin's pages to open;
-- `edges` are `{ from_id, to_id }`. `height` is in rems (12.5 by default).
function ui.graph(nodes, edges, height)
	local out = array({})
	for _, node in ipairs(nodes or {}) do
		table.insert(out, {
			id = tostring(node.id),
			label = node.label and tostring(node.label) or json.null,
			x = tonumber(node.x) or 0.5,
			y = tonumber(node.y) or 0.5,
			tone = tone(node.tone),
			focus = node.focus == true,
			to = node.page and { plugin = json.null, page = tostring(node.page), params = node.params or {} } or json.null,
		})
	end
	local lines = array({})
	for _, edge in ipairs(edges or {}) do
		table.insert(lines, { tostring(edge[1]), tostring(edge[2]) })
	end
	return { piece = "graph", nodes = out, edges = lines, height = tonumber(height) or 12.5 }
end

-- Columns of cards: a list of `{ id, title, cards = { { id, pieces } } }`.
-- Dragging a card to another column carries out the plugin's `action`
-- with `{ card, from, to }`: the card's id and the two columns'.
function ui.board(columns, action)
	local out = array({})
	for _, column in ipairs(columns or {}) do
		local cards = array({})
		for _, card in ipairs(column.cards or {}) do
			table.insert(cards, { id = tostring(card.id or card[1]), children = pieces(card.children or card[2]) })
		end
		table.insert(out, { id = tostring(column.id), title = tostring(column.title or column.id), cards = cards })
	end
	return { piece = "board", action = tostring(action), columns = out }
end

-- Tabs, one shown at a time: a list of `{ label, { pieces } }`. `id`
-- names them, so the tab the person chose is kept.
function ui.tabs(id, tabs)
	local out = array({})
	for _, tab in ipairs(tabs or {}) do
		table.insert(out, { label = tostring(tab[1]), children = pieces(tab[2]) })
	end
	return { piece = "tabs", id = tostring(id), tabs = out }
end

function ui.rule()
	return { piece = "rule" }
end

-- What shows where there is nothing yet.
function ui.empty(text)
	return { piece = "empty", text = tostring(text) }
end

-- Text that opens `page` of this plugin, or of `opts.plugin`, with
-- `opts.params`.
function ui.link(label, page, opts)
	opts = opts or {}
	local params = {}
	for key, value in pairs(opts.params or {}) do
		params[tostring(key)] = tostring(value)
	end
	return {
		piece = "link",
		label = tostring(label),
		to = { plugin = opts.plugin and tostring(opts.plugin) or json.null, page = tostring(page), params = params },
	}
end

-- Pieces bound to a key of the plugin's settings, for its settings page:
-- changing one saves that key, once the schema accepts the value.
-- A dotted key reaches into tables: `limits.max`.

function ui.toggle(key, label)
	return { piece = "toggle", key = tostring(key), label = label and tostring(label) or json.null }
end

-- One of `options`, or several with `{ multi = true }`.
function ui.choice(key, options, opts)
	local out = array({})
	for _, option in ipairs(options or {}) do
		table.insert(out, tostring(option))
	end
	return {
		piece = "choice",
		key = tostring(key),
		options = out,
		multi = (opts and opts.multi) == true,
	}
end

-- A line of text, or a number with `{ kind = "number" }`.
function ui.field(key, opts)
	opts = opts or {}
	local kind = opts.kind or "text"
	if kind ~= "text" and kind ~= "number" then
		error("a field's kind is text or number, not " .. tostring(kind), 2)
	end
	return {
		piece = "field",
		key = tostring(key),
		kind = kind,
		placeholder = opts.placeholder and tostring(opts.placeholder) or json.null,
	}
end

-- The run's status line: a short text, a detail, and a tone.
function ui.status(text, detail, of)
	return {
		piece = "status",
		text = tostring(text),
		detail = detail and tostring(detail) or json.null,
		tone = tone(of),
	}
end

-- `list` as a list of strings, or an error naming `what`.
function heiko._strings(list, what)
	local out = array({})
	for _, item in ipairs(list) do
		if type(item) ~= "string" then
			error(what .. " is a list of names", 0)
		end
		table.insert(out, item)
	end
	return out
end

-- The keys a plugin binds, `{ ["ctrl-2"] = "notes" }`: each to one of its
-- pages (ADR 0050, gap L).
function heiko._keys(keys, pages)
	local out = {}
	for key, page in pairs(keys) do
		if type(key) ~= "string" or type(page) ~= "string" then
			error("keys are `{ [key] = page }`, a page of its own for each key", 0)
		end
		if (pages or {})[page] == nil then
			error("the key `" .. key .. "` opens the page `" .. page .. "`, which `pages` lacks", 0)
		end
		out[key] = page
	end
	return out
end

-- Whether the page `name` that `what` opens is one of `pages`.
local function page_of(what, name, pages)
	if name ~= nil and (pages or {})[name] == nil then
		error("`" .. what .. "` opens the page `" .. tostring(name) .. "`, which `pages` lacks", 0)
	end
end

-- A tool's kind, one of `KINDS`, or nil when it declares none.
local function kind_of(name, kind)
	if kind ~= nil and not KINDS[kind] then
		error("the tool `" .. name .. "` has the kind `" .. tostring(kind) .. "`: a kind is look, search, change, run, reach or people", 0)
	end
	return kind
end

-- What the host reads of a plugin, without running its hooks.
function heiko._describe(plugin)
	page_of("nav", plugin.nav and plugin.nav.page, plugin.pages)
	page_of("repo_menu", plugin.repo_menu and (plugin.repo_menu.page or ""), plugin.pages)
	local tools = array({})
	for name, tool in pairs(plugin.tools or {}) do
		table.insert(tools, {
			name = name,
			description = tool.description or "",
			parameters = tool.parameters or { type = "object", properties = {} },
			card = tool.card ~= nil,
			-- What it does (ADR 0060): look, search, change, run, reach or people.
			kind = kind_of(name, tool.kind),
			-- The boolean setting the tool is offered while it is on.
			when = tool.when and tostring(tool.when) or nil,
			-- Only the model calls it, never a script.
			model_only = tool.model_only == true,
		})
	end
	table.sort(tools, function(a, b)
		return a.name < b.name
	end)
	local actions = array({})
	for name in pairs(plugin.actions or {}) do
		table.insert(actions, name)
	end
	table.sort(actions)
	local uses = plugin.uses or {}
	-- A folder is a name, read only, or `{ name, write = true }`, written
	-- too (ADR 0050, gap L).
	local files, writes = array({}), array({})
	for _, folder in ipairs(uses.files or {}) do
		if type(folder) == "table" then
			local name = folder[1]
			if type(name) ~= "string" then
				error("a folder of uses.files is a name, or { name, write = true }", 0)
			end
			table.insert(files, name)
			if folder.write == true then
				table.insert(writes, name)
			end
		else
			table.insert(files, folder)
		end
	end
	return {
		uses = {
			tools = uses.tools or array({}),
			decide = uses.decide == true,
			infer = uses.infer == true,
			network = uses.network or array({}),
			secrets = uses.secrets or array({}),
			model = uses.model or json.null,
			store = uses.store == true,
			transcript = uses.transcript == true,
			archive = uses.archive == true,
			chats = uses.chats == true,
			files = files,
			writes = writes,
		},
		-- What can be set; `view` draws the settings page, on the host.
		settings = plugin.settings and {
			schema = plugin.settings.schema or json.null,
			default = plugin.settings.default or json.null,
		} or json.null,
		tools = tools,
		hooks = {
			start = plugin.start ~= nil,
			before_tool = plugin.before_tool ~= nil,
			before_request = plugin.before_request ~= nil,
			after_tool_result = plugin.after_tool_result ~= nil,
			rewrite_context = plugin.rewrite_context ~= nil,
			rewritten = plugin.rewritten ~= nil,
			before_stop = plugin.before_stop ~= nil,
			turn_end = plugin.turn_end ~= nil,
			on_event = plugin.on_event ~= nil,
			title = plugin.title ~= nil,
			notify = plugin.notify ~= nil,
			repo_data = plugin.repo_data ~= nil,
			recover = plugin.recover ~= nil,
			run_end = plugin.run_end ~= nil,
			view = plugin.view ~= nil,
			settings_view = plugin.settings ~= nil and plugin.settings.view ~= nil,
			app = plugin.app ~= nil,
			page = plugin.page ~= nil,
			setup = plugin.setup ~= nil,
			models = plugin.models ~= nil,
		},
		listed_commands = plugin.listed ~= nil,
		actions = actions,
		events = plugin.events or array({}),
		commands = heiko._commands(plugin.commands),
		jobs = heiko._jobs(plugin.jobs),
		pages = heiko._names(plugin.pages),
		repo_nav = plugin.repo_nav and {
			label = plugin.repo_nav.label and tostring(plugin.repo_nav.label) or "",
			page = tostring(plugin.repo_nav.page or ""),
			icon = plugin.repo_nav.icon or "",
		} or json.null,
		fills = plugin.fills or array({}),
		compaction = plugin.compaction and {
			rewriters = plugin.compaction.rewriters or array({}),
		} or json.null,
		forge = plugin.forge and heiko._forge(plugin.forge) or json.null,
		nav = plugin.nav and {
			label = plugin.nav.label and tostring(plugin.nav.label) or "",
			icon = plugin.nav.icon or "",
			-- One of its pages of the app, which the entry opens (gap L).
			page = plugin.nav.page and tostring(plugin.nav.page) or nil,
		} or json.null,
		repo_menu = plugin.repo_menu and {
			label = plugin.repo_menu.label and tostring(plugin.repo_menu.label) or "",
			page = tostring(plugin.repo_menu.page or ""),
			icon = plugin.repo_menu.icon or "",
		} or nil,
		drafts = plugin.drafts and heiko._strings(plugin.drafts, "drafts") or nil,
		keys = plugin.keys and heiko._keys(plugin.keys, plugin.pages) or nil,
		picker_auto = plugin.picker_auto and {
			decide = tostring(plugin.picker_auto.decide),
			no_decide = tostring(plugin.picker_auto.no_decide),
		} or json.null,
		continues = heiko._continues(plugin.continues),
	}
end

local WEEKDAYS = { Mon = true, Tue = true, Wed = true, Thu = true, Fri = true, Sat = true, Sun = true }

-- One trigger of an agent as the host reads it, or why it is not one.
local function trigger_kind(name, trigger)
	if type(trigger) ~= "table" then
		error("trigger `" .. name .. "` is not a table", 0)
	end
	-- A poll has its own `every`; a schedule is `every` or `at`.
	local kinds = 0
	if trigger.poll ~= nil or trigger.every ~= nil or trigger.at ~= nil then
		kinds += 1
	end
	for _, key in ipairs({ "command", "stream", "mcp", "message" }) do
		if trigger[key] ~= nil then
			kinds += 1
		end
	end
	if kinds ~= 1 then
		error(
			"trigger `"
				.. name
				.. "` must be one of: a schedule (`every` or `at`), `command`, `poll`, `stream`, `mcp` or `message`",
			0
		)
	end
	if trigger.poll ~= nil then
		local poll = trigger.poll
		if type(poll) ~= "table" or (poll.tool == nil) == (poll.http == nil) then
			error("trigger `" .. name .. "`: `poll` is `{ tool = ..., args = ... }` or `{ http = { url = ... } }`", 0)
		end
		if type(trigger.every) ~= "string" then
			error("trigger `" .. name .. "`: a poll needs `every`", 0)
		end
		if type(trigger.key) ~= "string" and type(trigger.key) ~= "function" then
			error("trigger `" .. name .. "`: a poll needs `key`, a field or a function of an item", 0)
		end
		if trigger.items ~= nil and type(trigger.items) ~= "function" then
			error("trigger `" .. name .. "`: `items` is a function of the poll's result", 0)
		end
		if poll.tool ~= nil then
			if type(poll.tool) ~= "string" or poll.tool == "" then
				error("trigger `" .. name .. "`: `tool` is a tool's name", 0)
			end
			if poll.args ~= nil and type(poll.args) ~= "table" then
				error("trigger `" .. name .. "`: `args` is a table", 0)
			end
			return {
				kind = "poll",
				source = "tool " .. poll.tool,
				every = trigger.every,
				tool = poll.tool,
				args = poll.args or {},
			}
		end
		if type(poll.http) ~= "table" or type(poll.http.url) ~= "string" then
			error("trigger `" .. name .. "`: `http` is a request, `{ url = ..., headers = ... }`", 0)
		end
		-- The whole request, secrets' markers and all: the host makes it.
		return { kind = "poll", source = poll.http.url, every = trigger.every, http = poll.http }
	end
	if trigger.stream ~= nil then
		local stream = trigger.stream
		local shape = "trigger `" .. name .. "`: `stream` is `{ sse = url }` or `{ websocket = url }`"
		if type(stream) ~= "table" or (stream.sse == nil) == (stream.websocket == nil) then
			error(shape, 0)
		end
		for key in pairs(stream) do
			if key ~= "sse" and key ~= "websocket" and key ~= "headers" then
				error("trigger `" .. name .. "`: a stream has no `" .. tostring(key) .. "`", 0)
			end
		end
		local via = stream.sse ~= nil and "sse" or "websocket"
		local url = stream.sse or stream.websocket
		if type(url) ~= "string" then
			error(shape, 0)
		end
		local scheme = string.match(url, "^(%a+)://")
		local schemes = via == "sse" and { http = true, https = true } or { ws = true, wss = true }
		if not schemes[scheme] then
			error(
				"trigger `"
					.. name
					.. "`: "
					.. (via == "sse" and "server-sent events come from an http or https URL" or "a websocket is a ws or wss URL"),
				0
			)
		end
		if stream.headers ~= nil and type(stream.headers) ~= "table" then
			error("trigger `" .. name .. "`: `headers` is a table of names to values", 0)
		end
		if type(trigger.parse) ~= "function" then
			error("trigger `" .. name .. "`: a stream needs `parse`, a function of a frame", 0)
		end
		-- The headers, secrets' markers and all: the host opens it.
		return { kind = "stream", url = url, via = via, headers = stream.headers }
	end
	if trigger.mcp ~= nil then
		local mcp = trigger.mcp
		if type(mcp) ~= "table" or type(mcp.server) ~= "string" or type(mcp.uri) ~= "string" then
			error("trigger `" .. name .. "`: `mcp` is `{ server = ..., uri = ... }`", 0)
		end
		return { kind = "mcp", server = mcp.server, uri = mcp.uri }
	end
	if trigger.message ~= nil then
		if trigger.message ~= true then
			error("trigger `" .. name .. "`: `message` is `true`", 0)
		end
		return { kind = "message" }
	end
	if trigger.command ~= nil then
		if type(trigger.command) ~= "string" or trigger.command == "" then
			error("trigger `" .. name .. "`: `command` is the command's name", 0)
		end
		return {
			kind = "command",
			command = trigger.command,
			hint = trigger.hint and tostring(trigger.hint) or nil,
		}
	end
	local days = array({})
	for _, day in ipairs(trigger.days or {}) do
		if not WEEKDAYS[day] then
			error("trigger `" .. name .. "`: `" .. tostring(day) .. "` is not a day: Mon to Sun", 0)
		end
		table.insert(days, day)
	end
	if trigger.every ~= nil and trigger.at ~= nil then
		error("trigger `" .. name .. "`: a schedule is `every` or `at`, not both", 0)
	end
	if trigger.every ~= nil and trigger.days ~= nil then
		error("trigger `" .. name .. "`: `days` go with `at`", 0)
	end
	if trigger.every ~= nil and type(trigger.every) ~= "string" then
		error("trigger `" .. name .. "`: `every` is a duration, like \"15m\"", 0)
	end
	if trigger.at ~= nil and type(trigger.at) ~= "string" then
		error("trigger `" .. name .. "`: `at` is a time of day, like \"09:00\"", 0)
	end
	return { kind = "schedule", every = trigger.every, at = trigger.at, days = days }
end

-- How many keys a poll keeps: the host's too.
local POLL_KEYS = 10000

-- A poll's key of `item`: a field, or what a function of it gives.
local function poll_key(trigger, item)
	local key = trigger.key
	if type(key) == "function" then
		return key(item)
	end
	if type(item) ~= "table" then
		return nil
	end
	return item[key]
end

-- What one result of the poll `name` holds, as the host reads it: each
-- item with its key, in order; an item without a key is left out.
function heiko._poll_items(agent, name, result)
	local trigger = (agent.triggers or {})[name]
	if type(trigger) ~= "table" or trigger.poll == nil then
		error("the agent has no poll `" .. tostring(name) .. "`", 0)
	end
	local items = result
	if trigger.items ~= nil then
		items = trigger.items(result)
	end
	if type(items) ~= "table" then
		error("trigger `" .. name .. "`: the poll's items are a list, not " .. type(items), 0)
	end
	local out = array({})
	for _, item in ipairs(items) do
		local key = poll_key(trigger, item)
		if key ~= nil then
			table.insert(out, { key = tostring(key), item = item })
		end
	end
	return out
end

-- The event the stream `name`'s frame is, or nil, as `parse` gives it
-- and the host reads it: `{ id, data }`, the id as text.
local function parse_frame(name, trigger, frame, state)
	local event = trigger.parse(frame, state)
	if event == nil then
		return nil
	end
	if type(event) ~= "table" then
		error("trigger `" .. name .. "`: `parse` gives `{ id = ..., data = ... }` or nil, not " .. type(event), 0)
	end
	if type(event.id) ~= "string" and type(event.id) ~= "number" then
		error("trigger `" .. name .. "`: an event from `parse` needs an `id`", 0)
	end
	return { id = tostring(event.id), data = event.data }
end

-- What one frame of the stream `name` is, for the host: the event, if
-- any, and the stream's state as `parse` left it.
function heiko._parse_frame(agent, name, frame, state)
	local trigger = (agent.triggers or {})[name]
	if type(trigger) ~= "table" or type(trigger.parse) ~= "function" then
		error("the agent has no stream `" .. tostring(name) .. "`", 0)
	end
	local event = parse_frame(name, trigger, frame, state)
	return { event = event or json.null, state = state }
end

-- What the host reads of an agent: its name and what wakes it.
function heiko._describe_agent(agent)
	local names = {}
	for name in pairs(agent.triggers or {}) do
		if type(name) ~= "string" then
			error("a trigger's name is a string", 0)
		end
		table.insert(names, name)
	end
	table.sort(names)
	local triggers = array({})
	for _, name in ipairs(names) do
		table.insert(triggers, { name = name, kind = trigger_kind(name, agent.triggers[name]) })
	end
	-- The secrets its triggers name, for its page: the person fills them.
	local secrets, seen = array({}), {}
	local function walk(value, depth)
		if type(value) ~= "table" or depth > 16 then
			return
		end
		local name = rawget(value, "$secret")
		if type(name) == "string" then
			if not seen[name] then
				seen[name] = true
				table.insert(secrets, name)
			end
			return
		end
		for _, inner in pairs(value) do
			walk(inner, depth + 1)
		end
	end
	walk(agent.triggers, 0)
	table.sort(secrets)
	return {
		triggers = triggers,
		secrets = secrets,
	}
end

-- The `ctx` a hook gets, from what the host sends; a test gives its own
-- tools.
-- What a tool of the plugin's own answers, as text for the model: its
-- text, or its value as JSON.
local function tool_text(value)
	if type(value) == "string" then
		return value
	end
	-- `heiko.result(text)`.
	if type(value) == "table" and type(value.text) == "string" and type(value.flag) == "boolean" then
		return value.text
	end
	if value == nil then
		return "null"
	end
	return json.encode(value)
end

-- Commands, awaiting them, and a plugin's actor (ADR 0056).
require("heiko_actor")(heiko, { tool_text = tool_text })

-- The standard part (`plugins/heiko-std`): `heiko.text`, `heiko.count`,
-- `heiko.values`, `heiko.store`, `heiko.decide`, `heiko.infer` and `heiko.now`.
for _, part in ipairs({ "text", "count", "values", "store", "decide", "infer", "clock" }) do
	require("heiko_" .. part)(heiko)
end

-- For the test harness (`heiko_test`): reading a frame as a stream does,
-- and how many keys a poll keeps.
heiko._parse_one = parse_frame
heiko._POLL_KEYS = POLL_KEYS

return heiko
