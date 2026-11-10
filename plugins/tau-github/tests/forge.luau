-- The forge: who is signed in, the repositories the app reaches, page by
-- page, one of each and the most recently pushed first; whether it can
-- push to one; a pull request with its reviewers; and how its checks
-- stand.
local tau = require("tau")
local t = tau.test
local github = require("github")

local function route(routes)
	local asked = {}
	return function(request)
		local path = string.match(request.url, "^https?://[^/]+(/.*)$") or request.url
		table.insert(asked, { method = request.method, path = path, body = request.body })
		local answer = routes[request.method .. " " .. path]
		if type(answer) == "function" then
			return answer(request)
		end
		return answer or { status = 404, json = { message = "Not Found" } }
	end, asked
end

local function repo(name, pushed)
	return { full_name = name, default_branch = "main", pushed_at = pushed, clone_url = "https://github.com/" .. name .. ".git" }
end

t.case("the account is who signed in, none signed out", function()
	local r = t.run({ http = route({}), secrets = { github = true } })
	t.equal(r:forge("account", {}), json.null)
	r.store.account = { user = "octo" }
	t.equal(r:forge("account", {}), "octo")
end)

t.case("repositories come from the app's installations, one of each, the newest first", function()
	local http = route({
		["GET /user/installations?per_page=100&page=1"] = { json = { installations = { { id = 1 }, { id = 2 } } } },
		["GET /user/installations/1/repositories?per_page=100&page=1"] = {
			json = { repositories = { repo("a/old", "2026-01-01"), repo("a/new", "2026-11-01") } },
		},
		["GET /user/installations/2/repositories?per_page=100&page=1"] = {
			json = { repositories = { repo("a/new", "2026-11-01"), repo("b/mid", "2026-06-01") } },
		},
	})
	local r = t.run({ http = http, secrets = { github = true } })
	local names = {}
	for _, found in ipairs(r:forge("repos", {})) do
		table.insert(names, found.name)
	end
	t.equal(names, { "a/new", "b/mid", "a/old" })
end)

t.case("every page of repositories is read, and the person's own without installations", function()
	local first = {}
	for n = 1, github.PER_PAGE do
		table.insert(first, repo("o/r" .. n, string.format("2026-01-%02d", 1 + n % 28)))
	end
	local http, asked = route({
		["GET /user/installations?per_page=100&page=1"] = { status = 403, json = {} },
		["GET /user/repos?sort=pushed&per_page=100&page=1"] = { json = first },
		["GET /user/repos?sort=pushed&per_page=100&page=2"] = { json = { repo("o/last", "2026-12-01") } },
	})
	local r = t.run({ http = http, secrets = { github = true } })
	local found = r:forge("repos", {})
	t.equal(#found, github.PER_PAGE + 1)
	t.equal(found[1].name, "o/last")
	t.equal(asked[#asked].path, "/user/repos?sort=pushed&per_page=100&page=2")
end)

t.case("a failed page is an error", function()
	local http = route({
		["GET /user/installations?per_page=100&page=1"] = { json = { installations = { { id = 7 } } } },
		["GET /user/installations/7/repositories?per_page=100&page=1"] = { status = 502, json = {} },
	})
	local r = t.run({ http = http, secrets = { github = true } })
	local ok, why = pcall(r.forge, r, "repos", {})
	t.equal(ok, false)
	t.equal(string.find(tostring(why), "GitHub answered 502 for the repositories of installation 7", 1, true) ~= nil, true, why)
end)

t.case("can push is what GitHub says of the repository", function()
	local http = route({
		["GET /repos/o/yes"] = { json = { permissions = { push = true } } },
		["GET /repos/o/read"] = { json = { permissions = { push = false } } },
		["GET /repos/o/gone"] = { status = 404, json = {} },
		["GET /repos/o/bad"] = { status = 401, json = {} },
	})
	local r = t.run({ http = http, secrets = { github = true } })
	t.equal(r:forge("can_push", { repo = "o/yes" }), true)
	t.equal(r:forge("can_push", { repo = "o/read" }), false)
	t.equal(r:forge("can_push", { repo = "o/gone" }), false)
	local ok, why = pcall(r.forge, r, "can_push", { repo = "o/bad" })
	t.equal(ok, false)
	t.equal(string.find(tostring(why), "GitHub did not accept the token.", 1, true) ~= nil, true)
end)

t.case("a pull request opens and asks its reviewers", function()
	local http, asked = route({
		["POST /repos/o/r/pulls"] = { status = 201, json = { number = 42, html_url = "https://github.com/o/r/pull/42" } },
		["POST /repos/o/r/pulls/42/requested_reviewers"] = { status = 201, json = {} },
	})
	local r = t.run({ http = http, secrets = { github = true } })
	t.equal(
		r:forge("open_pull", { repo = "o/r", title = "T", body = "B", head = "h", base = "main", draft = false, reviewers = { "ann" } }),
		{ number = 42, url = "https://github.com/o/r/pull/42" }
	)
	t.equal(json.decode(asked[1].body), { title = "T", body = "B", head = "h", base = "main", draft = false })
	t.equal(json.decode(asked[2].body), { reviewers = { "ann" } })
	local refused = route({ ["POST /repos/o/r/pulls"] = { status = 403, json = { message = "Resource not accessible" } } })
	local ok, why = pcall(t.run({ http = refused, secrets = { github = true } }).forge, t.run({ http = refused, secrets = { github = true } }), "open_pull", { repo = "o/r", title = "T", body = "", head = "h", base = "main" })
	t.equal(ok, false)
	t.equal(
		string.find(
			tostring(why),
			"GitHub refused to open the pull request (403: Resource not accessible). tau's app needs contents and pull requests write access to the repository.",
			1,
			true
		) ~= nil,
		true,
		why
	)
end)

t.case("checks are none, running, failed or passed", function()
	local function checks(runs)
		local http = route({ ["GET /repos/o/r/commits/abc/check-runs"] = { json = { check_runs = runs } } })
		return t.run({ http = http, secrets = { github = true } }):forge("checks", { repo = "o/r", head = "abc" })
	end
	t.equal(checks({}), "none")
	t.equal(checks({ { status = "in_progress" } }), "running")
	t.equal(checks({ { status = "completed", conclusion = "success" }, { status = "in_progress" } }), "running")
	t.equal(checks({ { status = "completed", conclusion = "timed_out" }, { status = "in_progress" } }), "failed")
	t.equal(checks({ { status = "completed", conclusion = "success" } }), "passed")
end)

t.property("a form value encodes to unreserved characters and escapes, and back", function(draw)
	local g = t.gen
	local value = draw(g.one_of({ g.text({ max = 40 }), g.from_regex("[aZ09_.~:/ %%+&=-]{0,40}") }))
	local encoded = github.encode(value)
	t.equal(string.find(encoded, "[^A-Za-z0-9%-_%.~%%]") == nil, true, encoded)
	local decoded = string.gsub(encoded, "%%(%x%x)", function(hex)
		return string.char(tonumber(hex, 16))
	end)
	t.equal(decoded, value)
	for hex in string.gmatch(encoded, "%%(%x%x)") do
		t.equal(hex, string.upper(hex))
		local char = string.char(tonumber(hex, 16))
		t.equal(string.find(char, "[A-Za-z0-9%-_%.~]") == nil, true, "needless escape of " .. char)
	end
end)
