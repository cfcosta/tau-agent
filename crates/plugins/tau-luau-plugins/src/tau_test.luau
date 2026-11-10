-- The test harness every Luau plugin's and agent's tests run with
-- (ADR 0027, ADR 0049): cases, fake runs, generators, properties and
-- machines, as `tau.test`. Apart from `tau`, which every hook loads, so
-- that one stays small.

local tau = require("tau")

-- Tests: `tests/*.luau` files of cases, each run against a fake run.
local test = {}
tau.test = test
local cases = array({})

-- A case: its name and what it checks.
function test.case(name, check)
	table.insert(cases, { name = tostring(name), check = check })
end

-- Fixtures (ADR 0053): the value `make` returns, which the host holds to
-- `fixtures/<name>.json` and `tau-plugin test --bless` writes.
local fixtures = array({})

function test.fixture(name, make)
	table.insert(fixtures, { name = tostring(name), make = make })
end

-- Loops (ADR 0053): the host runs `spec`, data, as a real agent loop, and
-- gives `check` what the run came to. The spec and the run's fields are
-- tau-luau-plugins-host's `looping` module's: `prompt`, `steer`, `model`
-- (the turns), `settings`, `reasoning`, `clock`, `retry`, `plugins`,
-- `files`, `tools`, `jev` and `then` (forks, resumes, idle compaction);
-- `results`, `outcome`, `state`, `rewrites`, `logs`, `charged`,
-- `record`, `transcript`, `archive`, `forks`, `requests`, `most_at_once`,
-- `jev`, `workspace` and `blobs`.
local loops = array({})

function test.loop(name, spec, check)
	table.insert(loops, { name = tostring(name), spec = spec, check = check })
end

-- A workspace file of a loop's `spec.files`: a PNG of `width`×`height`,
-- or bytes given in base64.
function test.png(width, height)
	return { png = { width = width, height = height } }
end

function test.bytes(base64)
	return { base64 = base64 }
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

-- `unix` as `YYYY-MM-DD HH:MM` at `offset` seconds from UTC.
local function local_of(unix, offset)
	local at = unix + offset
	local days, seconds = at // 86400, at % 86400
	-- Howard Hinnant's civil_from_days.
	local z = days + 719468
	local era = z // 146097
	local doe = z - era * 146097
	local yoe = (doe - doe // 1460 + doe // 36524 - doe // 146096) // 365
	local doy = doe - (365 * yoe + yoe // 4 - yoe // 100)
	local mp = (5 * doy + 2) // 153
	local d = doy - (153 * mp + 2) // 5 + 1
	local m = mp < 10 and mp + 3 or mp - 9
	local y = yoe + era * 400 + (m <= 2 and 1 or 0)
	return string.format("%04d-%02d-%02d %02d:%02d", y, m, d, seconds // 3600, seconds % 3600 // 60)
end

-- `YYYY-MM-DD HH:MM` at `offset` seconds from UTC, in seconds since
-- 1970; nil when it is no such time.
local function parse_local(text, offset)
	local y, m, d, hh, mm = string.match(text, "^%s*(%d+)-(%d+)-(%d+) (%d+):(%d+)%s*$")
	if not y then
		return nil
	end
	y, m, d, hh, mm = tonumber(y), tonumber(m), tonumber(d), tonumber(hh), tonumber(mm)
	local leap = (y % 4 == 0 and y % 100 ~= 0) or y % 400 == 0
	local lengths = { 31, leap and 29 or 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31 }
	if m < 1 or m > 12 or d < 1 or d > lengths[m] or hh > 23 or mm > 59 then
		return nil
	end
	return unix_of(string.format("%04d-%02d-%02dT%02d:%02d:00", y, m, d, hh, mm)) - offset
end

-- A number, then `s`, `m`, `h` or `d`, in seconds; nil when it is none.
local function duration_of(text)
	local number, unit = string.match(text, "^%s*(%d+)([smhd])%s*$")
	number = tonumber(number)
	if not number or number <= 0 then
		return nil
	end
	return number * ({ s = 1, m = 60, h = 3600, d = 86400 })[unit]
end

local function deep_copy(value)
	if type(value) ~= "table" then
		return value
	end
	local out = {}
	for key, item in pairs(value) do
		out[key] = deep_copy(item)
	end
	-- An array's metatable is hidden: it reads as `false`.
	local meta = getmetatable(value)
	if meta == getmetatable(array({})) then
		return array(out)
	end
	return setmetatable(out, meta)
end

-- A fake run: `spec` may give `now` (an ISO date), `id`, `kind`, `repo`,
-- `settings`, `state`, `store` (a table of keys, shared by the runs
-- given the same one), `tools` (a function or a value per tool name),
-- `transcript` (the messages `ctx.transcript()` gives), `models` (what
-- `ctx.models` lists), `jev` (what `ctx.jev` is: functions as Jev's),
-- `files` (what `ctx.files` reads, by path: text, or `{ text, size,
-- image = { mime, width, height } }`; a blob is its path),
-- `offset` (the host's seconds from UTC, 0 by default), `stores` (by
-- repository, what a job's `ctx.stores` reads; the run's own is "test")
-- and `wake` (what `ctx.chats.wake` answers: a value, or a function of
-- the run and the text; "started" by default). Its hooks are methods;
-- each keeps the state the last one left, and a hook that fails keeps
-- none of what it wrote to the stores. `run.woken` lists each wake, as
-- `{ run, text }`.
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
	local offset = spec.offset or 0
	fakes.time_local = function(args)
		return local_of(args.at, offset)
	end
	fakes.time_parse = function(args)
		return parse_local(args.text, offset) or json.null
	end
	fakes.time_duration = function(args)
		return duration_of(args.text) or json.null
	end
	-- Every repository's store, the run's own among them.
	local stores = spec.stores or {}
	stores.test = kept
	local woken = {}
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
	-- What a tool reads with uses.files: `spec.files`, by path as the tool
	-- names it, each its text, or `{ text, image = { mime, width, height } }`.
	-- A blob is the file's path.
	if plugin.uses and plugin.uses.files then
		local files = spec.files or {}
		local function file(args)
			local found = files[args.path]
			if found == nil then
				error("Cannot read " .. args.path .. ": No such file or directory (os error 2)", 0)
			end
			if type(found) == "string" then
				return { text = found }
			end
			return found
		end
		fakes.files_read = function(args)
			local text = file(args).text or ""
			if args.limit then
				text = string.sub(text, 1, args.limit)
			end
			return text
		end
		fakes.files_list = function(args)
			local prefix = args.path == "." and "" or (args.path .. "/")
			local names = {}
			for path, found in pairs(files) do
				if string.sub(path, 1, #prefix) == prefix and not string.find(string.sub(path, #prefix + 1), "/") then
					local text = type(found) == "string" and found or (found.text or "")
					table.insert(names, { name = string.sub(path, #prefix + 1), dir = false, size = #text })
				end
			end
			table.sort(names, function(a, b)
				return a.name < b.name
			end)
			return array(names)
		end
		fakes.files_image = function(args)
			local found = file(args)
			local image = found.image and table.clone(found.image) or {}
			image.size = found.size or #(found.text or "")
			return image
		end
		fakes.blobs_put = function(args)
			local found = file(args)
			return { id = args.path, digest = "digest:" .. args.path, size_bytes = found.size or #(found.text or "") }
		end
	end
	-- What a job reaches with uses.chats: each repository's store, and
	-- waking chats. Only a job gets them: `job` says when one runs.
	local in_job = false
	local function job_only(fake)
		return function(args)
			if not in_job then
				error("only a job reaches every repository and wakes chats", 0)
			end
			return fake(args)
		end
	end
	if plugin.uses and plugin.uses.chats then
		local function store(args)
			stores[args.repo] = stores[args.repo] or {}
			return stores[args.repo]
		end
		fakes.repo_stores = job_only(function()
			local names = {}
			for name in pairs(stores) do
				table.insert(names, name)
			end
			table.sort(names)
			return array(names)
		end)
		fakes.repo_store_get = job_only(function(args)
			local value = store(args)[args.key]
			if value == nil then
				return json.null
			end
			return value
		end)
		fakes.repo_store_put = job_only(function(args)
			store(args)[args.key] = args.value
		end)
		fakes.repo_store_delete = job_only(function(args)
			store(args)[args.key] = nil
		end)
		fakes.repo_store_list = job_only(function(args)
			local from, keys = store(args), {}
			for key in pairs(from) do
				if string.sub(key, 1, #args.prefix) == args.prefix then
					table.insert(keys, key)
				end
			end
			table.sort(keys)
			local found = array({})
			for _, key in ipairs(keys) do
				table.insert(found, { key = key, value = from[key] })
			end
			return found
		end)
		fakes.chat_wake = job_only(function(args)
			table.insert(woken, { run = args.run, text = args.text })
			local answer = spec.wake
			if type(answer) == "function" then
				answer = answer(args.run, args.text)
			end
			return answer or "started"
		end)
	end
	-- The conversation `ctx.transcript()` gives: `spec.transcript`.
	if plugin.uses and plugin.uses.transcript then
		fakes.transcript = function()
			return spec.transcript or array({})
		end
	end
	local run = {
		store = kept,
		stores = stores,
		woken = woken,
		job_state = spec.job_state or {},
		state = spec.state or {},
		logs = {},
		data = {
			-- `live` and `jev`, as an action sees them: `spec.live`, and
			-- whether there is `spec.jev`.
			run = {
				id = spec.id or "test",
				kind = spec.kind or "main",
				repo = spec.repo or "test",
				model = "test",
				turn = 1,
				live = spec.live == true,
				jev = spec.jev ~= nil,
			},
			now = {
				unix = unix_of(now),
				iso = now,
				weekday = weekday(now),
				["local"] = local_of(unix_of(now), offset),
				offset = offset,
			},
			settings = spec.settings or defaults,
			models = spec.models or array({}),
		},
	}
	local function call(hook, ...)
		local data = table.clone(run.data)
		data.state = run.state
		local ctx = tau._ctx(data, fakes)
		if spec.jev then
			ctx.jev = spec.jev
		end
		local before = deep_copy(stores)
		local ok, answer = pcall(hook, ..., ctx)
		if not ok then
			for name, store in pairs(stores) do
				for key in pairs(store) do
					store[key] = nil
				end
				for key, value in pairs(before[name] or {}) do
					store[key] = value
				end
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
		local data, state = run.data, run.state
		-- A job is outside any run, with a state of its own: `run.job_state`.
		run.data = table.clone(data)
		run.data.run = {}
		run.state = run.job_state
		in_job = true
		local ok, answer = pcall(call, function(_, ctx)
			return job.run(ctx)
		end, nil)
		in_job, run.data = false, data
		run.job_state, run.state = run.state, state
		if not ok then
			error(answer, 0)
		end
		return answer
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
	-- `call` is the call's id, `ctx.run.call`: "call_1" when not given.
	function run:tool(name, args, call_id)
		local tool = need((plugin.tools or {})[name], "tool " .. name)
		local data = run.data
		run.data = table.clone(data)
		run.data.run = table.clone(data.run)
		run.data.run.call = call_id or "call_1"
		local ok, answer = pcall(call, tool.call, args or {})
		run.data = data
		if not ok then
			error(answer, 0)
		end
		return answer
	end
	-- Moves the run's clock to `iso`.
	function run:at(iso)
		run.data.now = {
			unix = unix_of(iso),
			iso = iso,
			weekday = weekday(iso),
			["local"] = local_of(unix_of(iso), offset),
			offset = offset,
		}
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
		return tau._parse_one(name, t, frame, fake.state[name])
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
		while #state.seen > tau._POLL_KEYS do
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

-- Every loop's name and spec, for the host to run.
function tau._loops()
	local out = array({})
	for _, loop in ipairs(loops) do
		table.insert(out, { name = loop.name, spec = loop.spec })
	end
	return out
end

-- The loop `name`'s check, on what its run came to.
function tau._check_loop(name, run)
	for _, loop in ipairs(loops) do
		if loop.name == name then
			local ok, why = pcall(loop.check, run)
			return { passed = ok, error = (not ok) and tostring(why) or json.null }
		end
	end
	return { passed = false, error = "there is no loop " .. name }
end

-- Makes every fixture, each on its own: its value, or why it failed.
function tau._run_fixtures()
	local made = array({})
	for _, fixture in ipairs(fixtures) do
		local ok, value = pcall(fixture.make)
		table.insert(made, {
			name = fixture.name,
			value = ok and value or json.null,
			error = (not ok) and tostring(value) or json.null,
		})
	end
	return made
end

return test
