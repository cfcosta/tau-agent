-- The `tau` module every Luau plugin requires (ADR 0027): the plugin's
-- declaration, the answers its hooks give, the view pieces, and what
-- the host uses to read a plugin and call its hooks.

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
	return spec
end

-- What `before_tool` answers.
function tau.block(reason)
	return { decision = "block", reason = tostring(reason) }
end

function tau.flag(reason)
	return { decision = "flag", reason = tostring(reason) }
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

function ui.code(lang, text)
	return { piece = "code", lang = lang and tostring(lang) or json.null, text = tostring(text) }
end

function ui.stack(children)
	return { piece = "stack", children = pieces(children) }
end

function ui.row(children)
	return { piece = "row", children = pieces(children) }
end

function ui.button(label, action, args)
	return { piece = "button", label = tostring(label), action = tostring(action), args = args or {} }
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
		},
		-- What can be set; `view` draws the settings page, on the host.
		settings = plugin.settings and {
			schema = plugin.settings.schema or json.null,
			default = plugin.settings.default or json.null,
		} or json.null,
		tools = tools,
		hooks = {
			before_tool = plugin.before_tool ~= nil,
			before_stop = plugin.before_stop ~= nil,
			turn_end = plugin.turn_end ~= nil,
			run_end = plugin.run_end ~= nil,
			view = plugin.view ~= nil,
			settings_view = plugin.settings ~= nil and plugin.settings.view ~= nil,
		},
		actions = actions,
	}
end

-- The `ctx` a hook gets, from what the host sends; a test gives its own
-- tools.
function tau._ctx(data, with_tools)
	local logs = array({})
	local ctx = {
		run = data.run,
		now = data.now,
		settings = data.settings or {},
		state = data.state or {},
		tools = with_tools or tools,
		jev = jev,
		_logs = logs,
	}
	function ctx.log(text)
		table.insert(logs, tostring(text))
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

-- A fake run: `spec` may give `now` (an ISO date), `kind`, `settings`,
-- `state` and `tools` (a function or a value per tool name). Its hooks
-- are methods; each keeps the state the last one left.
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
	local run = {
		state = spec.state or {},
		logs = {},
		data = {
			run = { id = "test", kind = spec.kind or "main", repo = "test", model = "test", turn = 1 },
			now = { unix = 0, iso = now, weekday = weekday(now) },
			settings = spec.settings or defaults,
		},
	}
	local function call(hook, ...)
		local data = table.clone(run.data)
		data.state = run.state
		local ctx = tau._ctx(data, fakes)
		local answer = hook(..., ctx)
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
	function run:card(name, c, result)
		local tool = need((plugin.tools or {})[name], "tool " .. name)
		local card = need(tool.card, "card for " .. name)
		return call(function(input, ctx)
			return card(input.call, input.result, ctx)
		end, { call = c, result = result })
	end
	return run
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
