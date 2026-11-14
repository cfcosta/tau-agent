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
-- A remote's URL as the host's `ctx.files.remote` normalises it: the
-- host lowercased, then the path, with no scheme, user, port, `.git` or
-- trailing slash; a local path stays a path.
function test.normalize_remote(url)
	url = string.match(url, "^%s*(.-)%s*$")
	if url == "" then
		return nil
	end
	local host, path
	local scheme_at = string.find(url, "://", 1, true)
	if scheme_at then
		local rest = string.sub(url, scheme_at + 3)
		local slash = string.find(rest, "/", 1, true)
		local authority = slash and string.sub(rest, 1, slash - 1) or rest
		path = slash and string.sub(rest, slash + 1) or ""
		host = string.match(authority, "([^@]*)$")
		host = string.match(host, "^([^:]*)")
	elseif string.sub(url, 1, 1) ~= "/" and string.find(url, ":", 1, true) then
		local colon = string.find(url, ":", 1, true)
		local authority = string.sub(url, 1, colon - 1)
		if not string.find(authority, "/", 1, true) then
			host = string.match(authority, "([^@]*)$")
			path = string.sub(url, colon + 1)
		else
			path = url
		end
	else
		path = url
	end
	path = string.match(path, "^/*(.-)/*$")
	if string.sub(path, -4) == ".git" then
		path = string.sub(path, 1, -5)
	end
	path = string.match(path, "^(.-)/*$")
	if host ~= nil and host ~= "" then
		return string.lower(host) .. "/" .. path
	end
	return "/" .. path
end

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
		-- Values go in and come out as copies, as the host's JSON does: a
		-- table read and then changed is not what the store keeps.
		fakes.store_get = function(args)
			local value = kept[args.key]
			if value == nil then
				return json.null
			end
			return deep_copy(value)
		end
		fakes.store_put = function(args)
			kept[args.key] = deep_copy(args.value)
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
				table.insert(found, { key = key, value = deep_copy(kept[key]) })
			end
			return found
		end
	end
	-- What a plugin with uses.network asks of the web: `spec.http`, a
	-- function of the request giving `{ status, json?, body? }`; `keep`
	-- takes its field out of `json` and sets the secret in `run.secrets`.
	-- `ctx.secrets` reads and empties `run.secrets`: `spec.secrets`, by
	-- name, whether each is set.
	local secrets = spec.secrets or {}
	if plugin.uses and plugin.uses.network then
		local answer = spec.http
		fakes.http = function(request)
			if answer == nil then
				error("this test's run gives no `http`", 0)
			end
			local got = answer(request)
			if type(got) ~= "table" then
				error("`spec.http` answers a table", 0)
			end
			got = table.clone(got)
			got.status = got.status or 200
			got.kept = false
			if request.keep and type(got.json) == "table" then
				got.json = table.clone(got.json)
				local value = got.json[request.keep.from]
				if type(value) == "string" and value ~= "" then
					got.json[request.keep.from] = nil
					secrets[request.keep.secret] = true
					got.kept = true
				end
			end
			return got
		end
	end
	if plugin.uses and plugin.uses.secrets then
		fakes.secret_has = function(args)
			return secrets[args.name] == true
		end
		fakes.secret_forget = function(args)
			secrets[args.name] = nil
			return json.null
		end
	end
	-- What the notify slot's `ctx.desktop.notice` shows: each notice in
	-- `run.notices`, `{ title, body }`, in order.
	local notices = array({})
	if plugin.notify then
		fakes.desktop_notice = function(args)
			table.insert(notices, { title = args.title, body = args.body or "" })
			return json.null
		end
	end
	-- What a tool reads with uses.files: `spec.files`, by path as the tool
	-- names it, each its text, or `{ text, image = { mime, width, height },
	-- modified }`, `modified` what a listing says of it (0 when not given).
	-- A blob is the file's path.
	local files = spec.files or {}
	local folders_made = {}
	if plugin.uses and plugin.uses.files then
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
			if args.offset then
				text = string.sub(text, math.min(args.offset, #text) + 1)
			end
			if args.limit then
				text = string.sub(text, 1, args.limit)
			end
			return text
		end
		fakes.files_list = function(args)
			local prefix = args.path == "." and "" or (string.gsub(args.path, "/$", "") .. "/")
			local names = {}
			local folders = {}
			for path, found in pairs(files) do
				if string.sub(path, 1, #prefix) == prefix then
					local rest = string.sub(path, #prefix + 1)
					local slash = string.find(rest, "/")
					if slash == nil then
						local text = type(found) == "string" and found or (found.text or "")
						local modified = type(found) == "table" and found.modified or 0
						table.insert(names, { name = rest, dir = false, size = #text, modified = modified })
					elseif not folders[string.sub(rest, 1, slash - 1)] then
						-- A folder is there when a file is under it.
						folders[string.sub(rest, 1, slash - 1)] = true
						table.insert(names, { name = string.sub(rest, 1, slash - 1), dir = true, size = 0, modified = 0 })
					end
				end
			end
			table.sort(names, function(a, b)
				return a.name < b.name
			end)
			return array(names)
		end
		-- A file's own `frontmatter`, else its text split at `---` lines
		-- with each `key: value` line a string: enough for a test's files.
		fakes.files_frontmatter = function(args)
			local found = file(args)
			if found.frontmatter then
				return found.frontmatter
			end
			local text = found.text or ""
			local name = string.match(args.path, "([^/]+)$") or args.path
			if string.sub(text, 1, 4) ~= "---\n" then
				return { error = name .. " does not start with frontmatter (---)" }
			end
			local close = string.find(text, "\n---\n", 4, true) or string.find(text, "\n---$", 4)
			if close == nil then
				return { error = name .. "'s frontmatter has no closing ---" }
			end
			local keys = array({})
			for line in string.gmatch(string.sub(text, 5, close), "([^\n]+)") do
				local key, value = string.match(line, "^([%w_%-]+):%s*(.-)%s*$")
				if key then
					table.insert(keys, { key = key, kind = value == "" and "null" or "string", text = value ~= "" and value or json.null })
				end
			end
			return { keys = keys, body = string.sub(text, close + 5) }
		end
		fakes.files_roots = function()
			return spec.roots or {}
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
		-- What it writes goes in `spec.files` too, by path as the plugin
		-- names it, for the test to read back (`run.files`); only with a
		-- folder `{ name, write = true }` (ADR 0050, gap L). A folder made
		-- is in `run.folders`.
		local writes = false
		for _, folder in ipairs(plugin.uses.files) do
			if type(folder) == "table" and folder.write == true then
				writes = true
			end
		end
		local function writable(path)
			if not writes then
				error("This plugin does not write " .. path .. ": its folder is not one `uses.files` names { name, write = true }", 0)
			end
		end
		fakes.files_write = function(args)
			writable(args.path)
			files[args.path] = args.text
		end
		fakes.files_remove = function(args)
			writable(args.path)
			if files[args.path] == nil then
				error("Cannot remove " .. args.path .. ": No such file or directory (os error 2)", 0)
			end
			files[args.path] = nil
		end
		fakes.files_rename = function(args)
			writable(args.path)
			writable(args.to)
			if files[args.path] == nil then
				error("Cannot move " .. args.path .. ": No such file or directory (os error 2)", 0)
			end
			files[args.to], files[args.path] = files[args.path], nil
		end
		fakes.files_mkdir = function(args)
			writable(args.path)
			folders_made[args.path] = true
		end
		-- `spec.remotes`: a folder, as the plugin names it, to its remote's
		-- URL; normalised as the host does.
		fakes.files_remote = function(args)
			local url = (spec.remotes or {})[args.path]
			if url == nil then
				return json.null
			end
			return { url = url, normalized = test.normalize_remote(url) }
		end
		-- `spec.repositories`: tau's folder for each repository, by name.
		fakes.files_repository = function(args)
			return (spec.repositories or {})[args.name] or json.null
		end
		-- `spec.repos`: the repositories tau lists, `{ id, name, folder,
		-- remote }` each, normalised as the host does (ADR 0050, gap L).
		fakes.files_repositories = function()
			local out = array({})
			for _, repo in ipairs(spec.repos or {}) do
				local listed = table.clone(repo)
				listed.id = listed.id or listed.name
				listed.normalized = listed.remote and test.normalize_remote(listed.remote) or json.null
				table.insert(out, listed)
			end
			return out
		end
	end
	-- `infer{ tools }` in rounds (ADR 0050, gap L): `spec.infer`, the
	-- answers taken in order, each `{ text, calls = { { name, args } } }`,
	-- or a function of the request giving one, or `{ error }`; each
	-- request is in `run.infers`, with what the tools answered before it.
	local infers = {}
	if plugin.uses and plugin.uses.infer then
		local answers = spec.infer or {}
		local next_answer = 1
		fakes.infer_turn = function(request)
			table.insert(infers, request)
			local answer
			if type(answers) == "function" then
				answer = answers(request)
			else
				answer = answers[next_answer]
				next_answer += 1
			end
			if answer == nil then
				answer = { text = "" }
			end
			if answer.error then
				return { ok = false, error = answer.error }
			end
			local calls = array({})
			for i, call in ipairs(answer.calls or {}) do
				table.insert(calls, { id = "call_" .. #infers .. "_" .. i, name = call.name, args = call.args or {} })
			end
			local text = answer.text or ""
			return { ok = true, value = { text = text, calls = calls, message = { text = text, calls = calls } } }
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
		-- The files, as the plugin wrote them, and the folders it made.
		files = files,
		folders = folders_made,
		-- Each round `infer{ tools }` asked.
		infers = infers,
		woken = woken,
		job_state = spec.job_state or {},
		state = spec.state or {},
		-- Which secrets are set, by name: what `ctx.secrets` and `keep` left.
		secrets = secrets,
		notices = notices,
		logs = {},
		-- Actions whose state was dropped: it changed under them.
		dropped = 0,
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
				ms = unix_of(now) * 1000,
			},
			settings = spec.settings or defaults,
			models = spec.models or array({}),
		},
	}
	-- `hook` with a ctx of the run's: `context.run` in place of the
	-- run's own `ctx.run`, the state in `run[context.slot]` ("state", or
	-- "job_state" for a job), and job reaches when `context.job`. Nothing
	-- of the run is swapped while it runs, so hooks under way at once
	-- (`t.hooks`) each keep their own.
	local function call_in(context, hook, ...)
		local slot = context.slot or "state"
		local data = table.clone(run.data)
		if context.run ~= nil then
			data.run = context.run
		end
		-- Its own copy, as on the host: what it leaves is the run's when it
		-- ends well and changed the state, over an action that came
		-- meanwhile; an action's is the run's only while the state is the
		-- one it began on (`still_on`), and is dropped otherwise.
		local started = deep_copy(run[slot])
		data.state = deep_copy(started)
		local was_job = in_job
		in_job = context.job == true
		local ctx = tau._ctx(data, fakes)
		if spec.jev then
			ctx.jev = spec.jev
		end
		local before = deep_copy(stores)
		local ok, answer = pcall(hook, ..., ctx)
		in_job = was_job
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
		if context.action then
			if same(run[slot], started) then
				run[slot] = ctx.state
			else
				run.dropped += 1
			end
		elseif not same(ctx.state, started) then
			run[slot] = ctx.state
		end
		for _, line in ipairs(ctx._logs) do
			table.insert(run.logs, line)
		end
		return answer
	end
	local function call(hook, ...)
		return call_in({}, hook, ...)
	end
	-- Whether a job's reaches are on: `t.hooks` keeps it for each hook it
	-- has under way.
	function run:_in_job(on)
		if on ~= nil then
			in_job = on
		end
		return in_job
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
	-- `f(ctx)` with the ctx a hook outside any run gets: for a test of a
	-- module the plugin's hooks call.
	function run:call(f)
		return call_in({ run = {} }, function(_, ctx)
			return f(ctx)
		end, nil)
	end
	function run:job(name)
		local job = need((plugin.jobs or {})[name], "job " .. name)
		-- A job is outside any run, with a state of its own: `run.job_state`.
		return call_in({ run = {}, slot = "job_state", job = true }, function(_, ctx)
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
		return call_in({ action = true }, need((plugin.actions or {})[name], "action " .. name), args or {})
	end
	-- `call` is the call's id, `ctx.run.call`: "call_1" when not given.
	function run:tool(name, args, call_id)
		local tool = need((plugin.tools or {})[name], "tool " .. name)
		local of = table.clone(run.data.run)
		of.call = call_id or "call_1"
		return call_in({ run = of }, tool.call, args or {})
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
	-- What it keeps of the app, the page it draws of it, and the commands
	-- it lists of it (ADR 0050, gap K).
	function run:app()
		return call(function(_, ctx)
			return need(plugin.app, "app")(ctx)
		end, nil)
	end
	function run:app_page(app)
		return call(need(plugin.page, "page"), app)
	end
	-- One of its forge's hooks (`tau.forge`), with `input`.
	function run:forge(name, input)
		local forge = need(plugin.forge, "forge")
		return call(need(forge[name], "forge." .. name), input or {})
	end
	-- Its step of first-run setup of what `app` keeps, with the
	-- repositories `picked` so far as `ctx.run.picked` (ADR 0050, gap L).
	function run:setup(app, picked)
		return call_in({ run = { kind = "app", picked = picked } }, need(plugin.setup, "setup"), app)
	end
	-- Its page `name` of the app, outside any repository (ADR 0050, gap
	-- L), with `params` and what its `drafts` hold, `params.drafts`.
	function run:app_page_of(name, params)
		local page = need((plugin.pages or {})[name], "page " .. name)
		return call_in({ run = { kind = "app" } }, page, table.clone(params or {}))
	end
	function run:models(app)
		return call(need(plugin.models, "models"), app)
	end
	function run:commands(app)
		local listed = need(plugin.listed, "listed commands")
		return call(function(input)
			return listed.list(input)
		end, app)
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
	properties[#properties].machine = true
end

-- `value` as Luau source: keys sorted, so the same value reads the same.
local function literal(value, depth)
	depth = depth or 0
	if type(value) == "string" then
		return string.format("%q", value)
	elseif type(value) ~= "table" then
		return tostring(value)
	end
	local keys = {}
	for key in pairs(value) do
		table.insert(keys, key)
	end
	table.sort(keys, function(a, b)
		return tostring(a) < tostring(b)
	end)
	local parts = {}
	local n = #value
	for i = 1, n do
		table.insert(parts, literal(value[i], depth + 1))
	end
	for _, key in ipairs(keys) do
		if type(key) ~= "number" or key < 1 or key > n or key % 1 ~= 0 then
			local name = type(key) == "string" and string.match(key, "^[%a_][%w_]*$") and key
				or "[" .. literal(key) .. "]"
			table.insert(parts, name .. " = " .. literal(value[key], depth + 1))
		end
	end
	return "{ " .. table.concat(parts, ", ") .. " }"
end

-- A machine of `tau.machine` (ADR 0054), tested on hegel's engine: one
-- rule per event, firing in every state, its data drawn from
-- `spec.events[name]`. After each step: no error, the state in shape,
-- an event with no arm changing nothing, effects well formed, the step
-- the same when taken again, and the state the same through JSON. A
-- plugin adds `spec.init(draw)` (its model), `spec.event(model, event,
-- state, effects)` (the model told each event) and `spec.invariant(state,
-- model, effects)`; `spec.weights` by event, and `spec.steps`. A failure
-- notes its events as a `t.case` to paste, stepping `spec.as` (the
-- machine's name in the plugin's code, `machine` unless said).
function test.machine_of(name, machine, spec, opts)
	spec = spec or {}
	local generators = spec.events or {}
	local function fails(m, why)
		local lines = {
			"t.case(" .. string.format("%q", tostring(name) .. ": found") .. ", function()",
			"\tlocal state",
			"\tfor _, event in ipairs(" .. literal(m.events) .. ") do",
			"\t\tstate = " .. (spec.as or "machine") .. ":step(state, event)",
			"\tend",
			"\t-- " .. string.gsub(tostring(why), "\n", " "),
			"end)",
		}
		test.note("the case, to keep:\n" .. table.concat(lines, "\n"))
		error(why, 0)
	end
	local function check(m, ok, why)
		if not ok then
			fails(m, why)
		end
	end
	local rules = {}
	for _, event in ipairs(machine:events()) do
		rules[event] = {
			weight = spec.weights and spec.weights[event] or 1,
			run = function(m, draw)
				local e = {}
				if generators[event] ~= nil then
					for key, value in pairs(draw(generators[event])) do
						e[key] = value
					end
				end
				e.event = event
				table.insert(m.events, deep_copy(e))
				local before = deep_copy(m.state)
				local handled = machine:handles(before, event)
				local ok, next, effects = pcall(machine.step, machine, deep_copy(before), deep_copy(e))
				check(m, ok, "the step raised: " .. tostring(next))
				if not handled then
					check(m, same(next, before), "`" .. event .. "` has no arm from `" .. before[machine.tag] .. "` and changed the state")
					check(m, #effects == 0, "`" .. event .. "` has no arm from `" .. before[machine.tag] .. "` and gave effects")
				end
				local again, again_effects = machine:step(deep_copy(before), deep_copy(e))
				check(m, same(again, next) and same(again_effects, effects), "`" .. event .. "` from `" .. before[machine.tag] .. "` gave another answer when taken again")
				check(m, same(json.decode(json.encode(next)), next), "the state does not read back the same from JSON: " .. shown(next))
				m.state = next
				m.effects = effects
				if spec.event then
					local fine, why = pcall(spec.event, m.model, e, next, effects)
					check(m, fine, why)
				end
			end,
		}
	end
	test.machine(name, {
		init = function(d)
			return {
				state = deep_copy(machine.initial),
				model = spec.init and spec.init(d) or {},
				effects = {},
				events = array({}),
			}
		end,
		rules = rules,
		invariant = function(m)
			local ok, why = machine:shape(m.state)
			check(m, ok, "the state " .. tostring(why))
			if spec.invariant then
				local fine, broke = pcall(spec.invariant, m.state, m.model, m.effects)
				check(m, fine, broke)
			end
		end,
		steps = spec.steps,
	}, opts)
end

-- A plugin through its ways in (ADR 0054), on hegel's engine. Each of
-- `spec.hooks` (a function of the fake run, `draw` and the world, or `{
-- weight, run }`) is a rule that starts that way in: a hook, an action, a
-- job's tick.
-- What it waits on pauses it: each function of `spec.run`'s `jev`,
-- `tools`, `http` and `infer`, called from a hook, waits until the rule
-- `answer` calls it, with `draw` after its arguments, or `fail` fails
-- it. So ways in can be under way at once, as on the host, sharing what
-- the host shares: a plugin's store is taken in turns (no way in starts
-- while another holds it), files and a run's state are shared at once.
-- A hook that raises fails the test, unless `spec.may_fail(hook, why)`.
-- `spec.invariant(world)` holds after each step: `world.run` (the fake
-- run, its stores, files and state), `world.under_way` (the ways in
-- waiting, each `{ hook, kind }`) and `world.failed`.
function test.hooks(name, spec, opts)
	local plugin = test.load()
	local stored = plugin.uses and plugin.uses.store
	local function waiting(kind, answer)
		return function(...)
			if not coroutine.isyieldable() then
				return answer(...)
			end
			local given = coroutine.yield({ kind = kind, args = table.pack(...), answer = answer })
			if not given.ok then
				error(given.why, 0)
			end
			return table.unpack(given.values, 1, given.values.n)
		end
	end
	local function paused(set, kind)
		if type(set) ~= "table" then
			return set
		end
		local out = {}
		for key, f in pairs(set) do
			out[key] = type(f) == "function" and waiting(kind .. "." .. key, f) or f
		end
		return out
	end
	-- Runs `way` on until it waits or ends, with the reaches it had.
	local function go_on(world, way, value)
		world.run:_in_job(way.in_job)
		local ok, out = coroutine.resume(way.co, value)
		way.in_job = world.run:_in_job()
		world.run:_in_job(false)
		if coroutine.status(way.co) ~= "dead" then
			way.wait = out
			table.insert(world.under_way, way)
			test.note("  " .. way.hook .. " waits on " .. out.kind)
			return
		end
		if ok then
			return
		end
		if out == ASSUMED then
			error(ASSUMED, 0)
		end
		if spec.may_fail and spec.may_fail(way.hook, out) then
			table.insert(world.failed, { hook = way.hook, why = tostring(out) })
			return
		end
		error(way.hook .. " raised: " .. tostring(out), 0)
	end
	local rules = {}
	for hook, entry in pairs(spec.hooks or {}) do
		if type(entry) == "function" then
			entry = { run = entry }
		end
		rules[hook] = {
			weight = entry.weight or 1,
			run = function(world, draw)
				test.assume(not (stored and #world.under_way > 0))
				local way = {
					hook = hook,
					in_job = false,
					co = coroutine.create(function()
						return entry.run(world.run, draw, world)
					end),
				}
				go_on(world, way)
			end,
		}
	end
	local function take(world, draw)
		test.assume(#world.under_way > 0)
		local at = draw(gen.integers({ min = 1, max = #world.under_way }))
		return table.remove(world.under_way, at)
	end
	rules.answer = {
		weight = 2,
		run = function(world, draw)
			local way = take(world, draw)
			local args = way.wait.args
			local ok, values = pcall(function()
				return table.pack(way.wait.answer(table.unpack(args, 1, args.n), draw))
			end)
			test.note("  " .. way.hook .. " gets its " .. way.wait.kind .. (ok and "" or ", which fails"))
			go_on(world, way, { ok = ok, values = values, why = values })
		end,
	}
	rules.fail = {
		weight = 0.5,
		run = function(world, draw)
			local way = take(world, draw)
			test.note("  " .. way.hook .. "'s " .. way.wait.kind .. " fails")
			go_on(world, way, { ok = false, why = "the " .. way.wait.kind .. " failed (t.hooks)" })
		end,
	}
	test.machine(name, {
		init = function()
			local given = table.clone(spec.run or {})
			given.jev = paused(given.jev, "jev")
			given.tools = paused(given.tools, "tool")
			if type(given.http) == "function" then
				given.http = waiting("http", given.http)
			end
			if type(given.infer) == "function" then
				given.infer = waiting("infer", given.infer)
			end
			return { run = test.run(given), under_way = array({}), failed = array({}) }
		end,
		rules = rules,
		invariant = spec.invariant,
		steps = spec.steps,
	}, opts)
end

-- The file's properties, for the host to run one at a time.
function tau._properties()
	local out = array({})
	for _, p in ipairs(properties) do
		table.insert(out, {
			name = p.name,
			cases = p.opts.cases or json.null,
			reproduce = p.opts.reproduce or json.null,
			machine = p.machine == true,
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

-- Runs every case, each on its own: what passed, why the rest failed,
-- and how long each took.
function tau._run_tests()
	local results = array({})
	for _, case in ipairs(cases) do
		local began = os.clock()
		local ok, why = pcall(case.check)
		table.insert(results, {
			name = case.name,
			passed = ok,
			error = (not ok) and tostring(why) or json.null,
			millis = math.floor((os.clock() - began) * 1000 + 0.5),
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
