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
		settings = plugin.settings or json.null,
		tools = tools,
		hooks = {
			before_tool = plugin.before_tool ~= nil,
			before_stop = plugin.before_stop ~= nil,
			turn_end = plugin.turn_end ~= nil,
			run_end = plugin.run_end ~= nil,
			view = plugin.view ~= nil,
		},
		actions = actions,
	}
end

-- The `ctx` a hook gets, from what the host sends.
function tau._ctx(data)
	local logs = array({})
	local ctx = {
		run = data.run,
		now = data.now,
		settings = data.settings or {},
		state = data.state or {},
		tools = tools,
		jev = jev,
		_logs = logs,
	}
	function ctx.log(text)
		table.insert(logs, tostring(text))
	end
	return ctx
end

return tau
