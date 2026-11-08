-- The `tau` module every Luau plugin and agent requires (ADR 0027,
-- ADR 0037): a plugin's or an agent's declaration, the answers a
-- plugin's hooks give, the view pieces, and what the host uses to read
-- them and call their hooks.

local tau = {}
local ui = {}
tau.ui = ui

-- A plugin: its table, checked.
function tau.plugin(spec)
	if type(spec) ~= "table" then
		error("tau.plugin takes a table", 2)
	end
	if type(spec.name) ~= "string" or spec.name == "" then
		error("a plugin needs a name", 2)
	end
	-- Each command runs as the action `/name`, given the text after it.
	for name, command in pairs(spec.commands or {}) do
		if type(command) ~= "table" or type(command.run) ~= "function" then
			error("command `" .. tostring(name) .. "` needs a `run`", 2)
		end
		spec.actions = spec.actions or {}
		spec.actions["/" .. name] = function(args, ctx)
			return command.run(args.text or "", ctx)
		end
	end
	return spec
end

-- An agent (ADR 0037): its table, checked. What wakes it is its
-- `triggers`, each by the name its events carry.
function tau.agent(spec)
	if type(spec) ~= "table" then
		error("tau.agent takes a table", 2)
	end
	if type(spec.name) ~= "string" or spec.name == "" then
		error("an agent needs a name", 2)
	end
	if spec.triggers ~= nil and type(spec.triggers) ~= "table" then
		error("an agent's `triggers` is a table of triggers by name", 2)
	end
	return spec
end

-- A secret by name, from tau's secret store: what an HTTP request's
-- headers or body may hold. The host puts the value in as the request
-- leaves; Luau never sees it. `"Bearer " .. tau.secret("github")` is the
-- secret with that text before its value, as is
-- `tau.secret("github", "Bearer ")`.
local Secret = {}

function tau.secret(name, prefix)
	if type(name) ~= "string" or name == "" then
		error("tau.secret takes a secret's name", 2)
	end
	if prefix ~= nil and type(prefix) ~= "string" then
		error("tau.secret's prefix is text", 2)
	end
	return setmetatable({ ["$secret"] = name, prefix = prefix }, Secret)
end

Secret.__concat = function(left, right)
	if type(left) == "string" and getmetatable(right) == Secret then
		return tau.secret(right["$secret"], left .. (right.prefix or ""))
	end
	error('a secret can only follow text, as in "Bearer " .. tau.secret("github")', 2)
end

-- A plugin's commands as the host reads them: each by name, with what it
-- does and what follows it, in name order.
function tau._commands(commands)
	local list = array({})
	for name, command in pairs(commands or {}) do
		table.insert(list, {
			name = name,
			description = command.description or "",
			args = command.args or "",
		})
	end
	table.sort(list, function(a, b)
		return a.name < b.name
	end)
	return list
end

-- The names of a table's functions, in order.
function tau._names(functions)
	local names = array({})
	for name in pairs(functions or {}) do
		table.insert(names, name)
	end
	table.sort(names)
	return names
end

-- A plugin's jobs as the host reads them: each by name, with how often
-- it runs and what it does, in name order.
function tau._jobs(jobs)
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
function tau._forge(forge)
	local hooks = array({})
	for _, name in ipairs({ "account", "repos", "can_push", "open_pull", "checks" }) do
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
function tau.start_chat(chat)
	return {
		tau_chat = {
			start = { repo = chat.repo, text = tostring(chat.text) },
			["then"] = chat["then"],
			args = chat.args or {},
		},
	}
end

-- What an action answers to have the interface do one of tau's button
-- actions once it is done, on the action's run: `composer` (`text`),
-- `send` (`text`), `steer` (`text`), `start` (`repo`, `text`), `alert`
-- (`title`, `message`) or `details`.
function tau.interface(action, args)
	return { tau_interface = { action = tostring(action), args = args or {} } }
end

-- What an action answers to say the person's `text` to the chat `run`,
-- steering a turn going or starting one; `then` and `args` as
-- `tau.start_chat`'s.
function tau.steer_chat(chat)
	return {
		tau_chat = {
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
function tau.allow(ask)
	return {
		tau_allow = {
			title = tostring(ask.title),
			text = ask.text and tostring(ask.text) or "",
			action = tostring(ask.action),
			args = ask.args or {},
		},
	}
end

-- What `before_tool` answers.
function tau.block(reason)
	return { decision = "block", reason = tostring(reason) }
end

function tau.flag(reason)
	return { decision = "flag", reason = tostring(reason) }
end

-- Lets the call run with `args` in place of its own, when the plugin
-- declares `uses.change_args`; the tool checks them as it would the
-- model's.
function tau.change(args)
	return { decision = "change", args = args }
end

-- What `start` answers: `context`, texts the run's first message
-- starts with, and `instructions`, added to the run's own.
function tau.start(plan)
	return {
		context = plan.context or array({}),
		instructions = plan.instructions or json.null,
	}
end

-- What `rewrite_context` answers to rewrite the context: `keep`, the
-- messages to keep by their `index`; `insert`, texts to put `before` a
-- message; `drops_conversation`, whether it drops what was said.
function tau.rewrite(rewrite)
	return {
		rewrite = {
			keep = rewrite.keep or array({}),
			insert = rewrite.insert or array({}),
			drops_conversation = rewrite.drops_conversation == true,
			details = rewrite.details or json.null,
		},
	}
end

-- What `after_tool_result` answers to have the model read `text` in
-- place of the call's result.
function tau.result(text)
	return { text = tostring(text) }
end

-- What `before_request` answers to set the turn's reasoning effort:
-- "low", "medium", "high" and so on.
function tau.effort(level)
	return { effort = tostring(level) }
end

-- What `before_stop` answers to go on.
function tau.continue(message)
	return { decision = "continue", message = tostring(message) }
end

-- Whether `list` holds `value`.
function tau.contains(list, value)
	for _, item in ipairs(list or {}) do
		if item == value then
			return true
		end
	end
	return false
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
function ui.toned(text, of, mono)
	return { piece = "toned", text = tostring(text), tone = tone(of), mono = mono == true }
end

function ui.stack(children)
	return { piece = "stack", children = pieces(children) }
end

function ui.row(children)
	return { piece = "row", children = pieces(children) }
end

local KINDS = { primary = true, secondary = true, danger = true }

-- A button that carries out the plugin's `actions[action]` with `args`;
-- `kind` is primary, secondary (the default) or danger.
function ui.button(label, action, args, kind)
	kind = kind or "secondary"
	if not KINDS[kind] then
		error("a button's kind is primary, secondary or danger, not " .. tostring(kind), 2)
	end
	return { piece = "button", label = tostring(label), action = tostring(action), args = args or {}, kind = kind }
end

function ui.heading(text)
	return { piece = "heading", text = tostring(text) }
end

-- Markdown, drawn as a model's reply is.
function ui.markdown(text)
	return { piece = "markdown", text = tostring(text) }
end

-- One of tau's icons, by name: `memory`, `sub-agent`, `check`.
function ui.icon(name, of)
	return { piece = "icon", name = tostring(name), tone = tone(of) }
end

function ui.pill(text, of)
	return { piece = "pill", text = tostring(text), tone = tone(of) }
end

-- A header over rows; each row is a list of pieces or strings.
function ui.table(head, rows)
	local h = array({})
	for _, cell in ipairs(head or {}) do
		table.insert(h, tostring(cell))
	end
	local out = array({})
	for _, row in ipairs(rows or {}) do
		table.insert(out, pieces(row))
	end
	return { piece = "table", head = h, rows = out }
end

-- Pieces that scroll past `height` rems (24 when not given).
function ui.scroll(children, height)
	return { piece = "scroll", children = pieces(children), height = height and tonumber(height) or json.null }
end

-- A heading that opens and closes `children`. `id` names it among the
-- plugin's sections, so where the person left it is kept; `opts.open`
-- says whether it starts open.
function ui.section(id, title, children, opts)
	return {
		piece = "section",
		id = tostring(id),
		title = tostring(title),
		open = (opts and opts.open) == true,
		children = pieces(children),
	}
end

-- A text input. `id` names it, and tau keeps what is typed in it, the
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
	}
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

-- A unified diff, colored as `path`'s language when given.
function ui.diff(text, path)
	return { piece = "diff", text = tostring(text), path = path and tostring(path) or json.null }
end

-- What a program wrote to a terminal, escapes and all. `id` names it, so
-- tau keeps its screen, scrolled where the person left it.
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

-- What the host reads of a plugin, without running its hooks.
function tau._describe(plugin)
	local tools = array({})
	for name, tool in pairs(plugin.tools or {}) do
		table.insert(tools, {
			name = name,
			description = tool.description or "",
			parameters = tool.parameters or { type = "object", properties = {} },
			card = tool.card ~= nil,
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
	return {
		name = plugin.name,
		description = plugin.description or "",
		uses = {
			tools = uses.tools or array({}),
			jev = uses.jev == true,
			infer = uses.infer == true,
			change_args = uses.change_args == true,
			network = uses.network or array({}),
			secrets = uses.secrets or array({}),
			model = uses.model or json.null,
			store = uses.store == true,
			transcript = uses.transcript == true,
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
		},
		actions = actions,
		events = plugin.events or array({}),
		commands = tau._commands(plugin.commands),
		jobs = tau._jobs(plugin.jobs),
		pages = tau._names(plugin.pages),
		repo_nav = plugin.repo_nav and {
			label = tostring(plugin.repo_nav.label or plugin.name),
			page = tostring(plugin.repo_nav.page or ""),
			icon = plugin.repo_nav.icon or "",
		} or json.null,
		fills = plugin.fills or array({}),
		compaction = plugin.compaction and {
			rewriters = plugin.compaction.rewriters or array({}),
		} or json.null,
		forge = plugin.forge and tau._forge(plugin.forge) or json.null,
		nav = plugin.nav and {
			label = tostring(plugin.nav.label or plugin.name),
			icon = plugin.nav.icon or "",
		} or json.null,
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
function tau._poll_items(agent, name, result)
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
function tau._parse_frame(agent, name, frame, state)
	local trigger = (agent.triggers or {})[name]
	if type(trigger) ~= "table" or type(trigger.parse) ~= "function" then
		error("the agent has no stream `" .. tostring(name) .. "`", 0)
	end
	local event = parse_frame(name, trigger, frame, state)
	return { event = event or json.null, state = state }
end

-- What the host reads of an agent: its name and what wakes it.
function tau._describe_agent(agent)
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
		name = agent.name,
		description = agent.description or "",
		triggers = triggers,
		secrets = secrets,
	}
end

-- The `ctx` a hook gets, from what the host sends; a test gives its own
-- tools.
-- The plugin's store (`uses.store`), over the host's calls `source`
-- has: none when it has no store.
local function store_of(source)
	if source == nil or source.store_get == nil then
		return nil
	end
	return {
		-- The value of `key`, or nil.
		get = function(key)
			local value = source.store_get({ key = tostring(key) })
			if value == json.null then
				return nil
			end
			return value
		end,
		put = function(key, value)
			source.store_put({ key = tostring(key), value = value })
		end,
		delete = function(key)
			source.store_delete({ key = tostring(key) })
		end,
		-- Every key starting with `prefix`, in key order, as { key, value }.
		list = function(prefix)
			return source.store_list({ prefix = tostring(prefix or "") })
		end,
	}
end

function tau._ctx(data, with_tools)
	local logs = array({})
	local ctx = {
		run = data.run,
		now = data.now,
		settings = data.settings or {},
		state = data.state or {},
		tools = with_tools or tools,
		store = store_of(with_tools or tools),
		jev = jev,
		-- The plan's models, newest of each family: `{ id, family, efforts }`.
		models = data.models or array({}),
		_logs = logs,
	}
	function ctx.log(text)
		table.insert(logs, tostring(text))
	end
	-- The run's conversation, whole: in `before_request` and
	-- `rewrite_context`, with `uses.transcript`.
	function ctx.transcript()
		local source = with_tools or tools
		if source == nil or source.transcript == nil then
			error("ctx.transcript() is for before_request and rewrite_context, with uses.transcript", 2)
		end
		return source.transcript({})
	end
	return ctx
end

-- Tests: `tests/*.luau` files of cases, each run against a fake run.
local test = {}
tau.test = test
local cases = array({})

-- A case: its name and what it checks.
function test.case(name, check)
	table.insert(cases, { name = tostring(name), check = check })
end

local function same(a, b)
	if type(a) ~= type(b) then
		return false
	end
	if type(a) ~= "table" then
		return a == b
	end
	for key, value in pairs(a) do
		if not same(value, b[key]) then
			return false
		end
	end
	for key in pairs(b) do
		if a[key] == nil then
			return false
		end
	end
	return true
end

local function shown(value)
	if type(value) == "table" then
		return json.encode(value)
	end
	return tostring(value)
end

-- Fails the case unless `got` and `want` are the same, tables compared
-- by what they hold.
function test.equal(got, want, what)
	if not same(got, want) then
		error((what and (what .. ": ") or "") .. "expected " .. shown(want) .. ", got " .. shown(got), 2)
	end
end

-- The plugin the tests are of: the test script hands it over, since a
-- module may only require what it declares.
function test.load()
	return test._plugin or error("no plugin under test", 2)
end

local DAYS = { "Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday" }

-- The weekday of an ISO date (`2026-10-09...`), by Sakamoto's method.
local function weekday(iso)
	local y, m, d = string.match(iso, "^(%d+)-(%d+)-(%d+)")
	y, m, d = tonumber(y), tonumber(m), tonumber(d)
	if not y then
		error("not an ISO date: " .. tostring(iso), 3)
	end
	local t = { 0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4 }
	if m < 3 then
		y = y - 1
	end
	local w = (y + y // 4 - y // 100 + y // 400 + t[m] + d) % 7
	return DAYS[w + 1]
end

-- Seconds since 1970 of an ISO time.
local function unix_of(iso)
	local y, m, d = string.match(iso, "^(%d+)-(%d+)-(%d+)")
	y, m, d = tonumber(y), tonumber(m), tonumber(d)
	if m <= 2 then
		y -= 1
	end
	local era = (y >= 0 and y or y - 399) // 400
	local yoe = y - era * 400
	local doy = (153 * ((m + 9) % 12) + 2) // 5 + d - 1
	local doe = yoe * 365 + yoe // 4 - yoe // 100 + doy
	local unix = (era * 146097 + doe - 719468) * 86400
	local hh, mm, ss = string.match(iso, "T(%d%d):(%d%d):(%d%d)")
	if hh then
		unix += tonumber(hh) * 3600 + tonumber(mm) * 60 + tonumber(ss)
	end
	return unix
end

local function deep_copy(value)
	if type(value) ~= "table" then
		return value
	end
	local out = {}
	for key, item in pairs(value) do
		out[key] = deep_copy(item)
	end
	return setmetatable(out, getmetatable(value))
end

-- A fake run: `spec` may give `now` (an ISO date), `id`, `kind`,
-- `settings`, `state`, `store` (a table of keys, shared by the runs
-- given the same one), `tools` (a function or a value per tool name)
-- and `transcript` (the messages `ctx.transcript()` gives). Its hooks are methods; each keeps the state the last one left,
-- and a hook that fails keeps none of what it wrote to the store.
function test.run(spec)
	spec = spec or {}
	local plugin = test.load()
	local defaults = plugin.settings and plugin.settings.default or {}
	local now = spec.now or "2026-01-05T09:00:00Z"
	local fakes = {}
	for name, fake in pairs(spec.tools or {}) do
		fakes[name] = function(args)
			if type(fake) == "function" then
				return fake(args)
			end
			return fake
		end
	end
	-- The store, kept in the fake run: `spec.store` to start from.
	local kept = spec.store or {}
	if plugin.uses and plugin.uses.store then
		fakes.store_get = function(args)
			local value = kept[args.key]
			if value == nil then
				return json.null
			end
			return value
		end
		fakes.store_put = function(args)
			kept[args.key] = args.value
		end
		fakes.store_delete = function(args)
			kept[args.key] = nil
		end
		fakes.store_list = function(args)
			local keys = {}
			for key in pairs(kept) do
				if string.sub(key, 1, #args.prefix) == args.prefix then
					table.insert(keys, key)
				end
			end
			table.sort(keys)
			local found = array({})
			for _, key in ipairs(keys) do
				table.insert(found, { key = key, value = kept[key] })
			end
			return found
		end
	end
	-- The conversation `ctx.transcript()` gives: `spec.transcript`.
	if plugin.uses and plugin.uses.transcript then
		fakes.transcript = function()
			return spec.transcript or array({})
		end
	end
	local run = {
		store = kept,
		state = spec.state or {},
		logs = {},
		data = {
			run = { id = spec.id or "test", kind = spec.kind or "main", repo = "test", model = "test", turn = 1 },
			now = { unix = unix_of(now), iso = now, weekday = weekday(now) },
			settings = spec.settings or defaults,
			models = spec.models or array({}),
		},
	}
	local function call(hook, ...)
		local data = table.clone(run.data)
		data.state = run.state
		local ctx = tau._ctx(data, fakes)
		local before = deep_copy(kept)
		local ok, answer = pcall(hook, ..., ctx)
		if not ok then
			for key in pairs(kept) do
				kept[key] = nil
			end
			for key, value in pairs(before) do
				kept[key] = value
			end
			error(answer, 0)
		end
		run.state = ctx.state
		for _, line in ipairs(ctx._logs) do
			table.insert(run.logs, line)
		end
		return answer
	end
	local function need(hook, name)
		if hook == nil then
			error("the plugin has no " .. name, 3)
		end
		return hook
	end
	function run:before_tool(c)
		return call(need(plugin.before_tool, "before_tool"), c)
	end
	function run:before_stop(stop)
		return call(need(plugin.before_stop, "before_stop"), stop)
	end
	function run:turn_end(turn)
		return call(need(plugin.turn_end, "turn_end"), turn)
	end
	function run:run_end(e)
		return call(need(plugin.run_end, "run_end"), e)
	end
	function run:start(plan)
		return call(need(plugin.start, "start"), plan)
	end
	function run:before_request(request)
		return call(need(plugin.before_request, "before_request"), request)
	end
	function run:after_tool_result(result)
		return call(need(plugin.after_tool_result, "after_tool_result"), result)
	end
	function run:rewrite_context(view)
		return call(need(plugin.rewrite_context, "rewrite_context"), view)
	end
	function run:rewritten(info)
		return call(need(plugin.rewritten, "rewritten"), info)
	end
	function run:on_event(event)
		return call(need(plugin.on_event, "on_event"), event)
	end
	function run:title(task)
		return call(need(plugin.title, "title"), task)
	end
	function run:notify(notice)
		return call(need(plugin.notify, "notify"), notice)
	end
	function run:job(name)
		local job = need((plugin.jobs or {})[name], "job " .. name)
		return call(function(_, ctx)
			return job.run(ctx)
		end, nil)
	end
	function run:view()
		local view = need(plugin.view, "view")
		return call(function(_, ctx)
			return view(ctx.state, ctx)
		end, nil)
	end
	-- The settings page, of `settings` or the run's.
	function run:settings_view(settings)
		local page = need(plugin.settings and plugin.settings.view, "settings.view")
		return call(function(input, ctx)
			return page(input, ctx)
		end, settings or run.data.settings)
	end
	function run:action(name, args)
		return call(need((plugin.actions or {})[name], "action " .. name), args or {})
	end
	function run:tool(name, args)
		local tool = need((plugin.tools or {})[name], "tool " .. name)
		return call(tool.call, args or {})
	end
	-- Moves the run's clock to `iso`.
	function run:at(iso)
		run.data.now = { unix = unix_of(iso), iso = iso, weekday = weekday(iso) }
		return run
	end
	function run:page(name, params)
		local page = need((plugin.pages or {})[name], "page " .. name)
		local given = table.clone(params or {})
		given.repo = given.repo or "test"
		return call(page, given)
	end
	function run:repo_data()
		return call(function(_, ctx)
			return need(plugin.repo_data, "repo_data")(ctx)
		end, nil)
	end
	function run:recover(input)
		return call(need(plugin.recover, "recover"), input)
	end
	function run:card(name, c, result)
		local tool = need((plugin.tools or {})[name], "tool " .. name)
		local card = need(tool.card, "card for " .. name)
		return call(function(input, ctx)
			return card(input.call, input.result, ctx)
		end, { call = c, result = result })
	end
	return run
end

-- The agent the tests are of, as the test script hands it over.
local function the_agent()
	return test._agent or error("no agent under test", 3)
end

-- A fake agent for tests (ADR 0037): `spec` may give `state`, by
-- trigger. Its methods run its triggers' Luau as the host would, with
-- no network.
function test.agent(spec)
	spec = spec or {}
	local agent = the_agent()
	local fake = { state = spec.state or {} }
	local function trigger(name)
		local found = (agent.triggers or {})[name]
		if found == nil then
			error("the agent has no trigger `" .. tostring(name) .. "`", 3)
		end
		return found
	end
	-- What it declares, as the host reads it.
	function fake:describe()
		return tau._describe_agent(agent)
	end
	-- The event a stream's frame is, or nil, as the host reads it: its
	-- id as text. The stream's state carries on from frame to frame.
	function fake:parse(name, frame)
		local t = trigger(name)
		if type(t.parse) ~= "function" then
			error("trigger `" .. name .. "` has no `parse`", 2)
		end
		fake.state[name] = fake.state[name] or {}
		return parse_frame(name, t, frame, fake.state[name])
	end
	-- The events a poll's result makes: one per item whose key it has
	-- not seen; the first poll only records keys.
	function fake:poll(name, result)
		trigger(name)
		local state = fake.state[name] or {}
		fake.state[name] = state
		local first = state.seen == nil
		local seen = {}
		for _, key in ipairs(state.seen or {}) do
			seen[tostring(key)] = true
		end
		state.seen = state.seen or {}
		local events = array({})
		for _, found in ipairs(tau._poll_items(agent, name, result)) do
			if not seen[found.key] then
				seen[found.key] = true
				table.insert(state.seen, found.key)
				if not first then
					table.insert(events, { id = name .. ":" .. found.key, data = found.item })
				end
			end
		end
		-- The host keeps the last 10 000 keys.
		while #state.seen > POLL_KEYS do
			table.remove(state.seen, 1)
		end
		return events
	end
	return fake
end

-- Property tests (ADR 0049): generators over hegel's engine. Every
-- choice is the engine's, asked of the case it offered through the host's
-- `hegel` tool; this only shapes what it gives.
local gen = {}
test.gen = gen

-- Raised by `assume` and a filter that gave up: the case is dropped.
local ASSUMED = setmetatable({}, {
	__tostring = function()
		return "assumed false"
	end,
})

local function hegel(args)
	if tools == nil or tools.hegel == nil then
		error("a draw happens only inside a property", 3)
	end
	local value = tools.hegel(args)
	if value == json.null then
		return nil
	end
	return value
end

local Gen = {}
Gen.__index = Gen

local function new(label, produce)
	return setmetatable({ label = label, produce = produce }, Gen)
end

-- `g`'s value, drawn in a span labelled with its kind: the engine
-- shrinks it as one piece.
local function draw(g)
	if getmetatable(g) ~= Gen then
		error("draw takes a generator, from t.gen", 2)
	end
	hegel({ op = "span_start", label = g.label })
	local value = g.produce(draw)
	hegel({ op = "span_stop" })
	return value
end

-- Whole numbers from `min` to `max`, both kept; unbounded ends stay
-- within what a Luau number holds exactly.
function gen.integers(opts)
	opts = opts or {}
	return new("integers", function()
		return hegel({ op = "integer", min = opts.min, max = opts.max })
	end)
end

-- Numbers from `min` to `max`: never NaN nor infinite.
function gen.floats(opts)
	opts = opts or {}
	return new("floats", function()
		return hegel({ op = "float", min = opts.min, max = opts.max })
	end)
end

-- `true` with probability `p` (one half).
function gen.booleans(opts)
	local p = opts and opts.p or 0.5
	return new("booleans", function()
		return hegel({ op = "boolean", p = p })
	end)
end

-- Text of `min` to `max` characters, from `codec` ("ascii", "latin-1"
-- or Unicode, the default).
function gen.text(opts)
	opts = opts or {}
	return new("text", function()
		return hegel({ op = "text", min = opts.min, max = opts.max, codec = opts.codec })
	end)
end

-- Text the whole of which matches `pattern`, in Python's `re` syntax.
function gen.from_regex(pattern)
	return new("from_regex", function()
		return hegel({ op = "regex", pattern = pattern })
	end)
end

function gen.just(value)
	return new("just", function()
		return value
	end)
end

-- One of `values`, the first ones first as it shrinks.
function gen.sampled_from(values)
	if #values == 0 then
		error("sampled_from needs a value", 2)
	end
	return new("sampled_from", function()
		return values[hegel({ op = "integer", min = 1, max = #values })]
	end)
end

-- A value of one of `gens`.
function gen.one_of(gens)
	if #gens == 0 then
		error("one_of needs a generator", 2)
	end
	return new("one_of", function(d)
		return d(gens[hegel({ op = "integer", min = 1, max = #gens })])
	end)
end

-- `g`'s value, or nil.
function gen.optional(g)
	return new("optional", function(d)
		if hegel({ op = "boolean", p = 0.5 }) then
			return d(g)
		end
		return nil
	end)
end

-- A list of `element`s, `min` to `max` of them, none twice when
-- `unique`. The engine says, one element at a time, whether there is
-- another, so shrinking takes elements out.
function gen.lists(element, opts)
	opts = opts or {}
	return new("lists", function(d)
		local out, seen = array({}), {}
		local list = hegel({ op = "collection", min = opts.min, max = opts.max })
		while hegel({ op = "more", id = list }) do
			local value = d(element)
			local key = opts.unique and json.encode(value)
			if key and seen[key] then
				hegel({ op = "reject", id = list })
			else
				if key then
					seen[key] = true
				end
				table.insert(out, value)
			end
		end
		return out
	end)
end

-- A table from keys of `keys` to values of `values`, `min` to `max`
-- entries.
function gen.dicts(keys, values, opts)
	opts = opts or {}
	return new("dicts", function(d)
		local out = {}
		local list = hegel({ op = "collection", min = opts.min, max = opts.max })
		while hegel({ op = "more", id = list }) do
			local key = d(keys)
			if out[key] ~= nil then
				hegel({ op = "reject", id = list })
			else
				out[key] = d(values)
			end
		end
		return out
	end)
end

-- A table with each field of `fields` drawn from its generator, in
-- sorted order: the same draws give the same record.
function gen.records(fields)
	local names = {}
	for name in pairs(fields) do
		table.insert(names, name)
	end
	table.sort(names)
	return new("records", function(d)
		local out = {}
		for _, name in ipairs(names) do
			out[name] = d(fields[name])
		end
		return out
	end)
end

-- A value `build(draw)` makes from other draws.
function gen.composite(build)
	return new("composite", build)
end

function Gen:map(f)
	return new("map", function(d)
		return f(d(self))
	end)
end

-- `self`'s values that `keep` keeps: three tries, then the case is
-- dropped.
function Gen:filter(keep)
	return new("filter", function(d)
		for _ = 1, 3 do
			local value = d(self)
			if keep(value) then
				return value
			end
		end
		error(ASSUMED, 0)
	end)
end

-- Drops the case unless `ok`: the engine looks elsewhere.
function test.assume(ok)
	if not ok then
		error(ASSUMED, 0)
	end
end

local notes = array({})

-- A line for the failure's report, after what it drew.
function test.note(text)
	table.insert(notes, tostring(text))
end

local function noted(value)
	if type(value) == "string" then
		return string.format("%q", value)
	end
	return shown(value)
end

local properties = array({})

-- A property: `check(draw)` holds for whatever it draws. `opts` may
-- give `cases` (100 unless said) and `reproduce`, a failure's blob to
-- replay.
function test.property(name, check, opts)
	table.insert(properties, { name = tostring(name), check = check, opts = opts or {} })
end

-- A state machine: `init(draw)` gives the model, each of `rules` (a
-- function of the model and `draw`, or `{ weight, run }`) changes it,
-- and `invariant(model)` holds after every step. `steps` is the most a
-- case takes (50).
function test.machine(name, spec, opts)
	local names = array({})
	for rule in pairs(spec.rules) do
		table.insert(names, rule)
	end
	table.sort(names)
	local weights = array({})
	local runs = {}
	for _, rule in ipairs(names) do
		local entry = spec.rules[rule]
		if type(entry) == "function" then
			entry = { run = entry }
		end
		table.insert(weights, entry.weight or 1)
		runs[rule] = entry.run
	end
	test.property(name, function(d)
		local model = spec.init and spec.init(d) or {}
		if spec.invariant then
			spec.invariant(model)
		end
		local machine = hegel({ op = "machine", rules = names, weights = weights, steps = spec.steps or 50 })
		local step = 0
		while hegel({ op = "round", id = machine }) do
			while true do
				local at = hegel({ op = "rule", id = machine })
				if at == nil then
					break
				end
				step += 1
				test.note("step " .. step .. ": " .. names[at])
				local ok, why = pcall(runs[names[at]], model, d)
				if not ok and why == ASSUMED then
					table.remove(notes)
					step -= 1
					hegel({ op = "rejected", id = machine })
				elseif not ok then
					error(why, 0)
				elseif spec.invariant then
					spec.invariant(model)
				end
			end
		end
	end, opts)
end

-- The file's properties, for the host to run one at a time.
function tau._properties()
	local out = array({})
	for _, p in ipairs(properties) do
		table.insert(out, {
			name = p.name,
			cases = p.opts.cases or json.null,
			reproduce = p.opts.reproduce or json.null,
		})
	end
	return out
end

-- One case of the property `name`, on the case the host holds: how it
-- ended, for the host to tell the engine.
local function run_case(name)
	local property
	for _, p in ipairs(properties) do
		if p.name == name then
			property = p
		end
	end
	if property == nil then
		return { status = "interesting", error = "no property " .. name, notes = notes }
	end
	local at = 0
	local function top(g)
		local value = draw(g)
		at += 1
		table.insert(notes, "draw " .. at .. " = " .. noted(value))
		return value
	end
	local ok, why = pcall(property.check, top)
	if ok then
		return { status = "valid" }
	end
	if why == ASSUMED then
		return { status = "invalid" }
	end
	return { status = "interesting", error = tostring(why), notes = notes }
end

-- Every case the engine offers of the property `name`, one after
-- another in this VM: what a case changes of the file's own values, the
-- next one sees.
function tau._run_cases(name)
	while hegel({ op = "next" }) do
		table.clear(notes)
		local outcome = run_case(name)
		hegel({ op = "end", status = outcome.status, error = outcome.error, notes = outcome.notes })
	end
	return { done = true }
end

-- Runs every case, each on its own: what passed, and why the rest
-- failed.
function tau._run_tests()
	local results = array({})
	for _, case in ipairs(cases) do
		local ok, why = pcall(case.check)
		table.insert(results, {
			name = case.name,
			passed = ok,
			error = (not ok) and tostring(why) or json.null,
		})
	end
	return results
end

return tau
