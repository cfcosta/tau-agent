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
	if spec.name ~= nil or spec.description ~= nil then
		error("a plugin's name and description are in its tau.toml", 2)
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
		spec.commands = nil
		spec.actions = setmetatable(spec.actions or {}, {
			__index = function(_, key)
				if type(key) == "string" and string.sub(key, 1, 1) == "/" then
					return function(args, ctx)
						ctx.drafts = args.drafts or {}
						return listed.run(string.sub(key, 2), args.text or "", ctx)
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
		spec.actions["/" .. name] = function(args, ctx)
			-- What the popover's inputs say, by id.
			ctx.drafts = args.drafts or {}
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
	if spec.name ~= nil or spec.description ~= nil then
		error("an agent's name and description are in its tau.toml", 2)
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
			icon = command.icon or "",
			popover = command.popover and {
				jev = command.popover.jev,
				no_jev = command.popover.no_jev or command.popover.jev,
			} or json.null,
		})
	end
	table.sort(list, function(a, b)
		return a.name < b.name
	end)
	return list
end

-- How many times a plugin's `before_stop` may keep a run going, as it
-- declares: a whole number from 1 to 20, or `json.null` for tau's own.
function tau._continues(continues)
	if continues == nil then
		return json.null
	end
	if type(continues) ~= "number" or continues % 1 ~= 0 or continues < 1 or continues > 20 then
		error("`continues` is a whole number from 1 to 20", 0)
	end
	return continues
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

-- What a tool answers to ask the person a form and wait for the
-- answers, from whichever device answers first: one to four
-- `questions`, each `{ question, header?, options = { { label,
-- description?, preview? } }, multi_select?, other? }`, `other` letting
-- the person write their own answer. The plugin's view draws the form
-- (`composer`); one of its actions gives the answers (`tau.answer`).
-- Then `actions[action]` is called in the run with `{ call, answers }`
-- or `{ call, declined = true }`, and the model reads what it answers.
function tau.ask(form)
	return {
		tau_ask = {
			questions = form.questions or array({}),
			action = form.action and tostring(form.action) or json.null,
		},
	}
end

-- What an action answers to give the call `call`, waiting on
-- `tau.ask`, the person's `answers` (each `{ picked = { labels },
-- other?, note? }`, in the questions' order), or with `declined = true`
-- that they would not answer. The first answer takes it; one that does
-- not fit the form, or comes once it is taken, is the action's error.
function tau.answer(answer)
	return {
		tau_answer = {
			call = tostring(answer.call),
			answers = answer.answers or json.null,
			declined = answer.declined == true,
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
-- starts with, `instructions`, added to the run's own, and `effort`, the
-- reasoning effort the run starts at when it is on auto.
function tau.start(plan)
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
function tau.fail(why)
	return { fail = tostring(why) }
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
function tau.result(text, options)
	return { text = tostring(text), flag = not (options and options.flag == false) }
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
-- `kind` is primary, secondary (the default) or danger. `icon`, one of
-- tau's icons by name, goes before the label, or alone with an empty one.
function ui.button(label, action, args, kind, icon)
	kind = kind or "secondary"
	if not KINDS[kind] then
		error("a button's kind is primary, secondary or danger, not " .. tostring(kind), 2)
	end
	return {
		piece = "button",
		label = tostring(label),
		action = tostring(action),
		args = args or {},
		kind = kind,
		icon = icon and tostring(icon) or nil,
	}
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
		-- A new number gives it the keys, once.
		focus = opts.focus and math.floor(tonumber(opts.focus) or 0) or nil,
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

-- Pieces in a frame.
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

-- What the host reads of a plugin, without running its hooks.
function tau._describe(plugin)
	local tools = array({})
	for name, tool in pairs(plugin.tools or {}) do
		table.insert(tools, {
			name = name,
			description = tool.description or "",
			parameters = tool.parameters or { type = "object", properties = {} },
			card = tool.card ~= nil,
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
	return {
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
			archive = uses.archive == true,
			chats = uses.chats == true,
			files = uses.files or array({}),
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
		},
		listed_commands = plugin.listed ~= nil,
		actions = actions,
		events = plugin.events or array({}),
		commands = tau._commands(plugin.commands),
		jobs = tau._jobs(plugin.jobs),
		pages = tau._names(plugin.pages),
		repo_nav = plugin.repo_nav and {
			label = plugin.repo_nav.label and tostring(plugin.repo_nav.label) or "",
			page = tostring(plugin.repo_nav.page or ""),
			icon = plugin.repo_nav.icon or "",
		} or json.null,
		fills = plugin.fills or array({}),
		compaction = plugin.compaction and {
			rewriters = plugin.compaction.rewriters or array({}),
		} or json.null,
		forge = plugin.forge and tau._forge(plugin.forge) or json.null,
		nav = plugin.nav and {
			label = plugin.nav.label and tostring(plugin.nav.label) or "",
			icon = plugin.nav.icon or "",
		} or json.null,
		picker_auto = plugin.picker_auto and {
			jev = tostring(plugin.picker_auto.jev),
			no_jev = tostring(plugin.picker_auto.no_jev),
		} or json.null,
		continues = tau._continues(plugin.continues),
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
		triggers = triggers,
		secrets = secrets,
	}
end

-- The `ctx` a hook gets, from what the host sends; a test gives its own
-- tools.
-- The plugin's store (`uses.store`), over the host's calls `source`
-- has: none when it has no store.
-- With `repo`, in a job: that repository's store, over the host's
-- `repo_store_*` calls.
local function store_of(source, repo)
	local prefix = repo and "repo_store_" or "store_"
	if source == nil or source[prefix .. "get"] == nil then
		return nil
	end
	local function call(name, args)
		args.repo = repo
		return source[prefix .. name](args)
	end
	return {
		-- The value of `key`, or nil.
		get = function(key)
			local value = call("get", { key = tostring(key) })
			if value == json.null then
				return nil
			end
			return value
		end,
		put = function(key, value)
			call("put", { key = tostring(key), value = value })
		end,
		delete = function(key)
			call("delete", { key = tostring(key) })
		end,
		-- Every key starting with `prefix`, in key order, as { key, value }.
		list = function(start)
			return call("list", { prefix = tostring(start or "") })
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
	-- The host's local time (ADR 0050, gap D): `local(at)` is `at`, in
	-- seconds since 1970, as `YYYY-MM-DD HH:MM` there; `parse(text)` reads
	-- one back, or nil when it is no such time; `duration(text)` is
	-- `90m`, `2h` or `3d` in seconds, or nil, as agents' schedules read
	-- them.
	local source = with_tools or tools
	ctx.time = {
		["local"] = function(at)
			return source.time_local({ at = math.floor(tonumber(at) or 0) })
		end,
		parse = function(text)
			local at = source.time_parse({ text = tostring(text) })
			if at == json.null then
				return nil
			end
			return at
		end,
		duration = function(text)
			local seconds = source.time_duration({ text = tostring(text) })
			if seconds == json.null then
				return nil
			end
			return seconds
		end,
	}
	-- In a job of a plugin with uses.chats: the repositories it keeps a
	-- store in, each one's store, and waking a chat.
	if source ~= nil and source.repo_stores ~= nil then
		function ctx.repos()
			return source.repo_stores({})
		end
		function ctx.stores(repo)
			return store_of(source, tostring(repo))
		end
	end
	-- In a tool's call, `start` and `app`, with uses.files: the folders it
	-- names, read-only, a path relative to the run's workspace, `~/` from
	-- home, or absolute.
	if source ~= nil and source.files_read ~= nil then
		ctx.files = {
			-- The file's text; with `{ bytes = true }` its bytes in base64;
			-- `limit`, the most bytes it reads.
			read = function(path, opts)
				opts = opts or {}
				return source.files_read({ path = tostring(path), bytes = opts.bytes == true, limit = opts.limit })
			end,
			-- What the folder holds: `{ name, dir, size }` each, by name.
			list = function(path)
				return source.files_list({ path = tostring(path) })
			end,
			-- The file's size in bytes, and the image it is by its bytes:
			-- `{ size, mime, width, height }`; `mime` is nil when it is no
			-- image tau draws (PNG, JPEG, GIF, WebP, BMP, SVG), `width` and
			-- `height` for an SVG.
			image = function(path)
				return source.files_image({ path = tostring(path) })
			end,
			-- A Markdown file's YAML frontmatter, between `---` lines at its
			-- start: `{ keys, body }`, each key `{ key, kind, text }` in
			-- order, `text` its value as written; or `{ error }` when it
			-- has none or it is no mapping (ADR 0050, gap K).
			frontmatter = function(path)
				return source.files_frontmatter({ path = tostring(path) })
			end,
			-- Where each folder `uses.files` names is, by its name there.
			roots = function()
				return source.files_roots({})
			end,
		}
		ctx.blobs = {
			-- Keeps the file's bytes, at most 8 MiB, in the plugin's blobs of
			-- the run's repository: `{ id, digest, size_bytes }`, for
			-- `ui.image`.
			put_file = function(path)
				return source.blobs_put({ path = tostring(path) })
			end,
		}
	end
	if source ~= nil and source.chat_wake ~= nil then
		ctx.chats = {
			-- Starts a turn of the chat `run` on tau's message `text`:
			-- "started", "busy" (a turn of it is going) or "gone" (it
			-- takes no more messages). What the job wrote to its stores
			-- is kept first.
			wake = function(run, text)
				return source.chat_wake({ run = tostring(run), text = tostring(text) })
			end,
		}
	end
	-- The run's conversation, whole: in `before_request` and
	-- `rewrite_context`, with `uses.transcript`.
	function ctx.transcript()
		local source = with_tools or tools
		if source == nil or source.transcript == nil then
			error(
				"ctx.transcript() is for before_request, after_tool_result and rewrite_context, with uses.transcript",
				2
			)
		end
		return source.transcript({})
	end
	-- In `rewrite_context`: how many bytes of JSON the transcript is,
	-- and would be after `rewrite`, as `tau.rewrite` takes it.
	function ctx.measure(rewrite)
		local source = with_tools or tools
		if source == nil or source.measure == nil then
			error("ctx.measure() is for rewrite_context", 2)
		end
		return source.measure(tau.rewrite(rewrite).rewrite)
	end
	-- With uses.archive: a new file's path, under tau's archive of the
	-- repository, and writing it, once, readable by its owner alone.
	ctx.archive = {
		path = function(kind)
			local source = with_tools or tools
			if source == nil or source.archive_path == nil then
				error("ctx.archive needs uses.archive", 2)
			end
			return source.archive_path({ kind = tostring(kind or "file") })
		end,
		write = function(path, text)
			local source = with_tools or tools
			if source == nil or source.archive_write == nil then
				error("ctx.archive needs uses.archive", 2)
			end
			source.archive_write({ path = tostring(path), text = tostring(text) })
		end,
	}
	return ctx
end

-- For the test harness (`tau_test`): reading a frame as a stream does,
-- and how many keys a poll keeps.
tau._parse_one = parse_frame
tau._POLL_KEYS = POLL_KEYS

return tau
