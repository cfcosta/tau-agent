-- GitHub's web and API: the device sign-in, the account, the
-- repositories the app reaches, whether it can push to one, pull
-- requests, and their checks, each a request awaited in the hook that
-- asks; the token is the `github` secret, which the plugin never sees.
local tau = require("tau")
local endpoints = require("endpoints")

-- One request to GitHub, awaited: what it answers, or an error.
local function request(r)
	return tau.await(tau.cmd.http.request(r))
end

local github = {}

-- The secret the sign-in's token is kept in.
github.SECRET = "github"

-- Items per page of a GitHub list: its maximum.
github.PER_PAGE = 100

-- Where to give the app access to more repositories.
function github.install_url()
	return "https://github.com/apps/" .. endpoints.app_slug .. "/installations/new"
end

-- `application/x-www-form-urlencoded` for one value.
function github.encode(value)
	return (
		string.gsub(value, "[^A-Za-z0-9%-_%.~]", function(char)
			return string.format("%%%02X", string.byte(char))
		end)
	)
end

local function form(fields)
	local parts = {}
	for _, field in ipairs(fields) do
		table.insert(parts, field[1] .. "=" .. github.encode(field[2]))
	end
	return table.concat(parts, "&")
end

-- A GitHub OAuth error, in words.
function github.explain(error, value)
	if error == "device_flow_disabled" then
		return "tau's GitHub App does not allow signing in with a code yet: turn on Device Flow in the app's settings, or use a personal access token."
	elseif error == "expired_token" then
		return "The code expired. Ask for a new one."
	elseif error == "access_denied" then
		return "The sign-in was cancelled on GitHub."
	end
	if type(value.error_description) == "string" then
		return value.error_description
	end
	return "GitHub said " .. error
end

-- A form posted to GitHub's web, as its OAuth takes one: the JSON it
-- answers. `keep` saves a field of it as a secret, past the plugin.
local function post_form(path, fields, keep)
	local ok, answer = pcall(request, {
		method = "POST",
		url = endpoints.web .. path,
		headers = {
			accept = "application/json",
			["content-type"] = "application/x-www-form-urlencoded",
		},
		body = form(fields),
		keep = keep,
	})
	if not ok then
		error("Cannot reach GitHub: " .. tostring(answer), 0)
	end
	if type(answer.json) ~= "table" then
		error("GitHub answered oddly: " .. tostring(answer.body), 0)
	end
	return answer.json, answer.kept
end

-- A request to GitHub's API with the sign-in's token: its status and the
-- JSON it answers (nil when it is none).
local function api(method, path, body)
	local headers = {
		accept = "application/vnd.github+json",
		authorization = "Bearer " .. tau.secret(github.SECRET),
		["x-github-api-version"] = "2022-11-28",
	}
	if body ~= nil then
		headers["content-type"] = "application/json"
	end
	local ok, answer = pcall(request, {
		method = method,
		url = endpoints.api .. path,
		headers = headers,
		body = body and json.encode(body) or nil,
	})
	if not ok then
		error("Cannot reach GitHub: " .. tostring(answer), 0)
	end
	return answer.status, answer.json
end

-- Starts a device sign-in: `{ device_code, user_code, verification_uri,
-- expires_in, interval }`.
function github.start_device()
	local value = post_form("/login/device/code", { { "client_id", endpoints.client_id } })
	if type(value.error) == "string" then
		error(github.explain(value.error, value), 0)
	end
	for _, name in ipairs({ "device_code", "user_code", "verification_uri" }) do
		if type(value[name]) ~= "string" then
			error("GitHub sent no `" .. name .. "`", 0)
		end
	end
	return {
		device_code = value.device_code,
		user_code = value.user_code,
		verification_uri = value.verification_uri,
		expires_in = type(value.expires_in) == "number" and value.expires_in or 900,
		interval = type(value.interval) == "number" and value.interval or 5,
	}
end

-- Where to enter a device code, without its scheme.
function github.address(uri)
	return (string.gsub(string.gsub(uri, "^https://", ""), "^http://", ""))
end

-- Polls a device sign-in: `{ pending = true }`, `{ slow_down = true }`,
-- `{ done = true, expires_in }` with the token kept as the secret, or
-- `{ failed = why }`.
function github.poll(device_code)
	local value, kept = post_form("/login/oauth/access_token", {
		{ "client_id", endpoints.client_id },
		{ "device_code", device_code },
		{ "grant_type", "urn:ietf:params:oauth:grant-type:device_code" },
	}, { secret = github.SECRET, from = "access_token" })
	if value.error == "authorization_pending" then
		return { pending = true }
	elseif value.error == "slow_down" then
		return { slow_down = true }
	elseif type(value.error) == "string" then
		return { failed = github.explain(value.error, value) }
	elseif kept then
		return { done = true, expires_in = type(value.expires_in) == "number" and value.expires_in or nil }
	end
	return { failed = "GitHub sent no token" }
end

-- The login the sign-in's token belongs to.
function github.user()
	local status, value = api("GET", "/user")
	if status == 401 then
		error("GitHub did not accept the token.", 0)
	end
	if type(value) == "table" and type(value.login) == "string" then
		return value.login
	end
	error("GitHub answered " .. tostring(status) .. " for the account", 0)
end

-- Every item of a paged list: `path` page by page, until a page comes
-- back short. `key` names the array in each page, or nil when the page
-- is it. Nil and the status of a page that failed.
local function all_pages(path, key)
	local join = string.find(path, "?", 1, true) and "&" or "?"
	local items = {}
	local page = 1
	while true do
		local status, value = api("GET", path .. join .. "per_page=" .. github.PER_PAGE .. "&page=" .. page)
		if status ~= 200 then
			return nil, status
		end
		local list = key and type(value) == "table" and value[key] or value
		if type(list) ~= "table" then
			list = {}
		end
		for _, item in ipairs(list) do
			table.insert(items, item)
		end
		if #list < github.PER_PAGE then
			return items
		end
		page += 1
	end
end

-- A repository as GitHub lists it, as the forge gives one.
local function forge_repo(repo)
	if type(repo.full_name) ~= "string" then
		return nil
	end
	return {
		name = repo.full_name,
		description = type(repo.description) == "string" and repo.description or "",
		branch = type(repo.default_branch) == "string" and repo.default_branch or "main",
		pushed_at = type(repo.pushed_at) == "string" and repo.pushed_at or "",
		remote = type(repo.clone_url) == "string" and repo.clone_url or nil,
	}
end

-- The repositories the sign-in reaches: through the app's installations,
-- else the person's own, each once, the most recently pushed first.
function github.repos()
	local found = {}
	local installations = all_pages("/user/installations", "installations")
	if installations then
		for _, installation in ipairs(installations) do
			if type(installation.id) == "number" then
				local id = string.format("%d", installation.id)
				local repositories, status =
					all_pages("/user/installations/" .. id .. "/repositories", "repositories")
				if repositories == nil then
					error(
						"GitHub answered "
							.. tostring(status)
							.. " for the repositories of installation "
							.. id,
						0
					)
				end
				for _, repo in ipairs(repositories) do
					local made = forge_repo(repo)
					if made then
						table.insert(found, made)
					end
				end
			end
		end
	else
		local repositories, status = all_pages("/user/repos?sort=pushed", nil)
		if repositories == nil then
			error("GitHub answered " .. tostring(status) .. " for your repositories", 0)
		end
		for _, repo in ipairs(repositories) do
			local made = forge_repo(repo)
			if made then
				table.insert(found, made)
			end
		end
	end
	-- One of each, the first found of a name, the most recently pushed
	-- first.
	local seen, unique = {}, {}
	for at, repo in ipairs(found) do
		if not seen[repo.name] then
			seen[repo.name] = true
			table.insert(unique, { at = at, repo = repo })
		end
	end
	table.sort(unique, function(a, b)
		if a.repo.pushed_at ~= b.repo.pushed_at then
			return a.repo.pushed_at > b.repo.pushed_at
		end
		if a.repo.name ~= b.repo.name then
			return a.repo.name < b.repo.name
		end
		return a.at < b.at
	end)
	local sorted = {}
	for _, item in ipairs(unique) do
		table.insert(sorted, item.repo)
	end
	return sorted
end

-- Whether the sign-in can push to `full_name`.
function github.can_push(full_name)
	local status, value = api("GET", "/repos/" .. full_name)
	if status == 403 or status == 404 then
		return false
	elseif status == 401 then
		error("GitHub did not accept the token.", 0)
	elseif status < 200 or status >= 300 then
		error("GitHub answered " .. tostring(status) .. " for " .. full_name, 0)
	end
	-- The person's own right first: `permissions` is theirs, not the app's.
	if not (type(value) == "table" and type(value.permissions) == "table" and value.permissions.push == true) then
		return false
	end
	return github.app_writes(full_name)
end

-- Whether one of the app's installations reaches `full_name` and may
-- write its contents: what a push through the sign-in needs, as the
-- token is the app's on the person's behalf.
function github.app_writes(full_name)
	local installations = all_pages("/user/installations", "installations")
	if installations == nil then
		return false
	end
	for _, installation in ipairs(installations) do
		local permissions = type(installation.permissions) == "table" and installation.permissions or {}
		if permissions.contents == "write" and type(installation.id) == "number" then
			local id = string.format("%d", installation.id)
			local repositories = all_pages("/user/installations/" .. id .. "/repositories", "repositories") or {}
			for _, repo in ipairs(repositories) do
				if repo.full_name == full_name then
					return true
				end
			end
		end
	end
	return false
end

-- Why a push to `full_name` is refused, when GitHub refuses it (403):
-- the sign-in is the app's, which pushes only where it is installed and
-- may write.
function github.push_refused(full_name)
	return "GitHub refused the push to "
		.. full_name
		.. ": tau signs in through its GitHub app, which pushes only to repositories it is installed on with read and write access to their contents. Add "
		.. full_name
		.. " to the app, or give it write access, at "
		.. github.install_url()
end

-- A request that must succeed: its answer, or why GitHub would not `what`.
local function expect(method, path, body, what)
	local status, value = api(method, path, body)
	if status >= 200 and status < 300 then
		return value
	end
	local message = type(value) == "table" and type(value.message) == "string" and value.message or "no reason given"
	if status == 403 or status == 404 then
		error(
			"GitHub refused to "
				.. what
				.. " ("
				.. tostring(status)
				.. ": "
				.. message
				.. "). tau's app needs contents and pull requests write access to the repository.",
			0
		)
	end
	error("GitHub could not " .. what .. " (" .. tostring(status) .. ": " .. message .. ")", 0)
end

-- Opens a pull request, and asks its reviewers for reviews: its number and
-- page.
function github.open_pull(pull)
	local value = expect("POST", "/repos/" .. pull.repo .. "/pulls", {
		title = pull.title,
		body = pull.body,
		head = pull.head,
		base = pull.base,
		draft = pull.draft == true,
	}, "open the pull request")
	local number = type(value) == "table" and type(value.number) == "number" and value.number or 0
	local reviewers = pull.reviewers or {}
	if #reviewers > 0 then
		expect(
			"POST",
			"/repos/" .. pull.repo .. "/pulls/" .. string.format("%d", number) .. "/requested_reviewers",
			{ reviewers = reviewers },
			"ask for reviews"
		)
	end
	return { number = number, url = type(value) == "table" and type(value.html_url) == "string" and value.html_url or "" }
end

-- How the check runs on `commit` stand: "none", "failed", "passed" or
-- "running".
function github.checks(repo, commit)
	local status, value = api("GET", "/repos/" .. repo .. "/commits/" .. commit .. "/check-runs")
	if status ~= 200 then
		error("GitHub answered " .. tostring(status) .. " for the checks", 0)
	end
	local runs = type(value) == "table" and type(value.check_runs) == "table" and value.check_runs or {}
	if #runs == 0 then
		return "none"
	end
	local failed, done = false, true
	for _, run in ipairs(runs) do
		local conclusion = run.conclusion
		if
			conclusion == "failure"
			or conclusion == "timed_out"
			or conclusion == "cancelled"
			or conclusion == "action_required"
		then
			failed = true
		end
		if run.status ~= "completed" then
			done = false
		end
	end
	if failed then
		return "failed"
	elseif done then
		return "passed"
	end
	return "running"
end

return github
