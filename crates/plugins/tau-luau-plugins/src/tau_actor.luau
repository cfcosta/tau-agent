-- Commands, awaiting them, and a plugin's actor (ADR 0056), installed
-- into `tau` by the `tau` module: `tau.cmd`, `tau.await`, `tau.all`,
-- `tau.give`, `tau.log` and `tau._actor`.

return function(tau, helpers)
local tool_text = helpers.tool_text

-- Commands (ADR 0056): every effect a hook has, as data. A hook awaits
-- one with `tau.await`, which gives its answer, or gives one that needs
-- none with `tau.give`. The host carries them out; `op` is the host call
-- it makes, with `args`.
local cmd = {}
tau.cmd = cmd

local function command(op, args)
	return { op = op, args = args or {} }
end

cmd.store = {
	get = function(key)
		return command("store_get", { key = tostring(key) })
	end,
	list = function(prefix)
		return command("store_list", { prefix = tostring(prefix or "") })
	end,
	put = function(key, value)
		return command("store_put", { key = tostring(key), value = value })
	end,
	delete = function(key)
		return command("store_delete", { key = tostring(key) })
	end,
}

-- A repository's store, in a job of a plugin with uses.chats.
function cmd.stores(repo)
	repo = tostring(repo)
	return {
		get = function(key)
			return command("repo_store_get", { repo = repo, key = tostring(key) })
		end,
		list = function(prefix)
			return command("repo_store_list", { repo = repo, prefix = tostring(prefix or "") })
		end,
		put = function(key, value)
			return command("repo_store_put", { repo = repo, key = tostring(key), value = value })
		end,
		delete = function(key)
			return command("repo_store_delete", { repo = repo, key = tostring(key) })
		end,
	}
end

function cmd.repos()
	return command("repo_stores")
end

cmd.files = {
	read = function(path, opts)
		opts = opts or {}
		return command("files_read", {
			path = tostring(path),
			bytes = opts.bytes == true,
			limit = opts.limit,
			offset = opts.offset,
			lossy = opts.lossy == true,
		})
	end,
	list = function(path)
		return command("files_list", { path = tostring(path) })
	end,
	image = function(path)
		return command("files_image", { path = tostring(path) })
	end,
	frontmatter = function(path)
		return command("files_frontmatter", { path = tostring(path) })
	end,
	roots = function()
		return command("files_roots")
	end,
	remote = function(path)
		return command("files_remote", { path = tostring(path) })
	end,
	repositories = function()
		return command("files_repositories")
	end,
	repository = function(name)
		return command("files_repository", { name = tostring(name) })
	end,
	write = function(path, text)
		return command("files_write", { path = tostring(path), text = tostring(text) })
	end,
	remove = function(path)
		return command("files_remove", { path = tostring(path) })
	end,
	rename = function(from, to)
		return command("files_rename", { path = tostring(from), to = tostring(to) })
	end,
	mkdir = function(path)
		return command("files_mkdir", { path = tostring(path) })
	end,
}

cmd.blobs = {
	put_file = function(path)
		return command("blobs_put", { path = tostring(path) })
	end,
}

cmd.time = {
	["local"] = function(at)
		return command("time_local", { at = math.floor(tonumber(at) or 0) })
	end,
	parse = function(text)
		return command("time_parse", { text = tostring(text) })
	end,
	duration = function(text)
		return command("time_duration", { text = tostring(text) })
	end,
}

cmd.secrets = {
	has = function(name)
		return command("secret_has", { name = tostring(name) })
	end,
	forget = function(name)
		return command("secret_forget", { name = tostring(name) })
	end,
}

cmd.http = {
	request = function(request)
		return command("http", request)
	end,
}

-- Jev, as the `jev` global took it: `ask`, `noul`, `choice` and `score`
-- answer with `(answer, cost)`.
cmd.jev = {}
for _, name in ipairs({ "ask", "noul", "choice", "score" }) do
	cmd.jev[name] = function(request)
		return command("jev", { fn = name, request = request })
	end
end

function cmd.transcript()
	return command("transcript")
end

function cmd.measure(rewrite)
	return command("measure", tau.rewrite(rewrite).rewrite)
end

cmd.archive = {
	path = function(kind)
		return command("archive_path", { kind = tostring(kind or "file") })
	end,
	write = function(path, text)
		return command("archive_write", { path = tostring(path), text = tostring(text) })
	end,
}

function cmd.notice(notice)
	return command("desktop_notice", {
		title = tostring(notice.title),
		body = if notice.body == nil then "" else tostring(notice.body),
	})
end

cmd.chats = {
	wake = function(run, text)
		return command("chat_wake", { run = tostring(run), text = tostring(text) })
	end,
}

-- The ops that are not a host tool's name.
local OPS = { all = true, jev = true }

-- A host tool `uses.tools` names, with its args: its name is the op, as
-- every host call's is.
function cmd.tool(name, args)
	name = tostring(name)
	if OPS[name] then
		error("`" .. name .. "` is a command's op, not a tool's name", 2)
	end
	return command(name, args)
end

-- The model: one answer, or with `tools`, rounds in which it calls the
-- plugin's own tools, each called in the awaiting hook, until it calls
-- none of `wait_for` (every tool unless said) or `rounds` (4) are done.
function cmd.infer(request)
	return command("infer", request)
end

-- An answer's values, JSON's null as nil: a key the store does not have
-- reads as nil.
local function unpack_values(values)
	values = values or {}
	local out = {}
	local n = 0
	for i, value in pairs(values) do
		if type(i) == "number" then
			n = math.max(n, i)
			out[i] = if value == json.null then nil else value
		end
	end
	return table.unpack(out, 1, n)
end

-- The model answers in an envelope, `{ ok, value, error }`: an await
-- gives its value, or raises its error, as every other command's.
local ENVELOPED = { infer = true, infer_turn = true }

local function unwrap(values, level)
	local envelope = unpack_values(values)
	if type(envelope) ~= "table" then
		return envelope
	end
	if envelope.ok == false then
		error(envelope.error or "the model gave no answer", level + 1)
	end
	if envelope.value == json.null then
		return nil
	end
	return envelope.value
end

-- What awaiting needs to know of the step it runs in.
local current = nil

local AWAIT = {}

-- The answer to `command`, once the host gives it: the values it
-- answers with, or an error when it failed. Only a hook may await, and
-- not in a coroutine of its own, which would take the yield as its own.
function tau.await(c)
	if type(c) ~= "table" or type(c.op) ~= "string" then
		error("tau.await takes a command, from tau.cmd", 2)
	end
	if current == nil or coroutine.running() ~= current.thread then
		error("tau.await is for a hook, not a view nor a coroutine of its own", 2)
	end
	if c.op == "infer" and c.args.tools ~= nil then
		return tau._infer_with_tools(c.args)
	end
	local answer = coroutine.yield(AWAIT, c)
	if answer.ok == false then
		error(answer.error, 2)
	end
	if ENVELOPED[c.op] then
		return unwrap(answer.values, 2)
	end
	return unpack_values(answer.values)
end

-- Every command's answer, in order, as `{ ok, values, error }` each: they
-- are carried out at once.
function tau.all(commands)
	if current == nil or coroutine.running() ~= current.thread then
		error("tau.all is for a hook", 2)
	end
	local answers = coroutine.yield(AWAIT, { op = "all", args = array(commands) })
	return answers.values
end

-- Carries `command` out after the step, needing no answer.
function tau.give(c)
	if type(c) ~= "table" or type(c.op) ~= "string" then
		error("tau.give takes a command, from tau.cmd", 2)
	end
	if current == nil then
		error("tau.give is for a hook", 2)
	end
	table.insert(current.given, c)
end

-- A line in the plugin's log.
function tau.log(text)
	if current == nil then
		error("tau.log is for a hook", 2)
	end
	table.insert(current.logs, tostring(text))
end

-- `infer{ tools }` on commands: each round awaits the model, and each
-- tool it calls runs here, in the hook that awaits it.
function tau._infer_with_tools(args)
	local own = tau._spec and tau._spec.tools or {}
	local offered = array({})
	local named = {}
	for _, entry in ipairs(args.tools) do
		local name, tool = entry, own[entry]
		if type(entry) == "table" then
			name, tool = entry.name, entry
		end
		if type(tool) ~= "table" or type(tool.call) ~= "function" or type(name) ~= "string" then
			error("infer offers only the plugin's own tools: it has no `" .. tostring(name) .. "`", 3)
		end
		named[name] = tool
		table.insert(offered, {
			name = name,
			description = tool.description or "",
			parameters = tool.parameters or { type = "object", properties = {} },
		})
	end
	local waits = {}
	for _, entry in ipairs(args.wait_for or args.tools) do
		waits[type(entry) == "table" and entry.name or entry] = true
	end
	local request = {}
	for key, value in pairs(args) do
		if key ~= "tools" and key ~= "wait_for" and key ~= "rounds" then
			request[key] = value
		end
	end
	request.tools = offered
	request.turns = array({})
	local calls = array({})
	local text = ""
	for _ = 1, args.rounds or 4 do
		local answer = coroutine.yield(AWAIT, command("infer_turn", request))
		if answer.ok == false then
			error(answer.error, 2)
		end
		local turn_answer = unwrap(answer.values, 2)
		text = turn_answer.text or ""
		local asked = turn_answer.calls or {}
		if #asked == 0 then
			break
		end
		local turn = { message = turn_answer.message, results = array({}) }
		local waited = false
		for _, call in ipairs(asked) do
			local tool = named[call.name]
			local ok, value
			if tool == nil then
				ok, value = false, "no tool named " .. tostring(call.name)
			else
				ok, value = pcall(tool.call, call.args or {}, current.model, current.env)
				if not ok then
					value = tostring(value)
				end
			end
			if waits[call.name] then
				waited = true
			end
			table.insert(calls, { name = call.name, args = call.args or {}, ok = ok })
			table.insert(turn.results, { id = call.id, name = call.name, text = tool_text(value), ok = ok })
		end
		table.insert(request.turns, turn)
		if not waited then
			break
		end
	end
	return text, calls
end

-- The plugin's hook at `path` (keys from the plugin's table), or nil.
local function hook_at(plugin, path)
	local at = plugin
	for _, key in ipairs(path) do
		if type(at) ~= "table" then
			return nil
		end
		at = at[key]
	end
	return at
end

-- An actor (ADR 0056): a plugin's model and its hooks under way, taking
-- one message a step. `deliver(msg)` takes:
--
-- - `{ kind = "call", id, path, input, env }`: starts the hook at `path`
--   with `(input, model, env)`;
-- - `{ kind = "answer", id, ok, values, error }`: the answer to what the
--   hook `id` awaits;
-- - `{ kind = "cancel", id, why }`: ends the hook `id`, its await
--   raising `why`.
--
-- It gives what the step did: `model`, `changed`, `ended` (by hook id,
-- `{ value }` or `{ error }`), `waits` (by hook id, the command it now
-- awaits), `given` (commands needing no answer, in order) and `logs`.
-- A step that fails keeps none of what it changed of the model.
function tau._actor(plugin, model)
	local actor = { model = model or {}, under_way = {} }
	-- The model as it was, in place: hooks under way hold this table.
	local function restore(before)
		local was = json.decode(before)
		table.clear(actor.model)
		for key, value in pairs(was) do
			actor.model[key] = value
		end
	end
	local function run_step(id, thread, env, ...)
		local before = json.encode(actor.model)
		local step = { thread = thread, given = array({}), logs = array({}), model = actor.model, env = env }
		local was = current
		current = step
		local results = table.pack(coroutine.resume(thread, ...))
		current = was
		local out = {
			ended = {},
			waits = {},
			given = step.given,
			logs = step.logs,
		}
		if not results[1] then
			-- The step failed: its changes to the model go, and the hook ends.
			restore(before)
			actor.under_way[id] = nil
			out.ended[id] = { error = tostring(results[2]) }
			out.given = array({})
		elseif coroutine.status(thread) == "dead" then
			actor.under_way[id] = nil
			local value = results[2]
			if value == nil then
				value = json.null
			end
			out.ended[id] = { value = value }
		elseif results[2] == AWAIT then
			actor.under_way[id] = { thread = thread, env = env, waits = results[3] }
			out.waits[id] = results[3]
		else
			-- It yielded something else: a coroutine.yield of its own.
			restore(before)
			actor.under_way[id] = nil
			out.ended[id] = { error = "a hook yields only through tau.await" }
			out.given = array({})
		end
		local after = json.encode(actor.model)
		out.model = actor.model
		out.changed = after ~= before
		return out
	end
	function actor.deliver(msg)
		if msg.kind == "call" then
			if actor.under_way[msg.id] ~= nil then
				error("hook " .. tostring(msg.id) .. " is under way already")
			end
			-- The harness gives the hook itself; the host, its path.
			local hook = msg.hook or hook_at(plugin, msg.path or {})
			if type(hook) ~= "function" then
				return {
					model = actor.model,
					changed = false,
					ended = { [msg.id] = { value = json.null } },
					waits = {},
					given = array({}),
					logs = array({}),
				}
			end
			local thread = coroutine.create(function(input, env)
				return hook(input, actor.model, env)
			end)
			return run_step(msg.id, thread, msg.env or {}, msg.input, msg.env or {})
		elseif msg.kind == "answer" or msg.kind == "cancel" then
			local way = actor.under_way[msg.id]
			if way == nil then
				error("no hook " .. tostring(msg.id) .. " awaits")
			end
			local answer = msg
			if msg.kind == "cancel" then
				answer = { ok = false, error = tostring(msg.why or "cancelled") }
			end
			return run_step(msg.id, way.thread, way.env, answer)
		end
		error("no message kind " .. tostring(msg.kind))
	end
	-- The hooks under way, by id, and what each awaits.
	function actor.waiting()
		local out = {}
		for id, way in pairs(actor.under_way) do
			out[id] = way.waits
		end
		return out
	end
	-- What `view` (or another drawing at `path`) draws from a frozen copy
	-- of the model: drawing waits on nothing.
	function actor.draw(path, input, env)
		return actor.draw_with(hook_at(plugin, path), input, env)
	end
	-- `draw`, of the function `hook`: `view(model, env)`, or with an
	-- input, `hook(input, model, env)`.
	function actor.draw_with(hook, input, env)
		if type(hook) ~= "function" then
			return json.null
		end
		local frozen = json.decode(json.encode(actor.model))
		local function freeze(value)
			if type(value) == "table" and not table.isfrozen(value) then
				for _, item in pairs(value) do
					freeze(item)
				end
				table.freeze(value)
			end
			return value
		end
		if input == nil then
			return hook(freeze(frozen), env or {})
		end
		return hook(input, freeze(frozen), env or {})
	end
	return actor
end

end
