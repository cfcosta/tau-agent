-- Signing in: a device code polled at GitHub's interval until the person
-- approves, the token kept as the `github` secret past the plugin; a typed
-- token checked once, a refused one forgotten; an expired one forgotten;
-- and what the setup step and the Models screen draw of it.
local tau = require("tau")
local t = tau.test

local START = "2026-11-10T12:00:00Z"

-- A fake GitHub: each request is answered by `routes`, by its method and
-- the path after the host, and kept in `asked`.
local function fake(routes)
	local asked = {}
	return function(request)
		local path = string.match(request.url, "^https?://[^/]+(/.*)$") or request.url
		table.insert(asked, request.method .. " " .. path)
		local route = routes[request.method .. " " .. path]
		if route == nil then
			return { status = 404, json = { message = "Not Found" } }
		end
		if type(route) == "function" then
			return route(request, #asked)
		end
		return route
	end, asked
end

local DEVICE = {
	json = {
		device_code = "dev-1",
		user_code = "ABCD-1234",
		verification_uri = "https://github.com/login/device",
		expires_in = 900,
		interval = 5,
	},
}

local USER = { json = { login = "octo" } }

local function run(spec)
	spec.now = spec.now or START
	return t.run(spec)
end

local function find(tree, kind, found)
	found = found or {}
	if type(tree) == "table" then
		if tree.piece == kind then
			table.insert(found, tree)
		end
		for key, value in pairs(tree) do
			if key ~= "piece" then
				find(value, kind, found)
			end
		end
	end
	return found
end

local function labels(tree)
	local out = {}
	for _, button in ipairs(find(tree, "button")) do
		table.insert(out, button.label)
	end
	return out
end

t.case("a device sign-in polls until approved, the token past the plugin", function()
	local polls = 0
	local http, asked = fake({
		["POST /login/device/code"] = DEVICE,
		["POST /login/oauth/access_token"] = function(request)
			polls += 1
			t.equal(request.keep, { secret = "github", from = "access_token" })
			if polls == 1 then
				return { json = { error = "authorization_pending" } }
			elseif polls == 2 then
				return { json = { error = "slow_down" } }
			end
			return { json = { access_token = "gho_token", expires_in = 28800 } }
		end,
		["GET /user"] = USER,
	})
	local r = run({ http = http })
	r:action("sign_in", {})
	local app = r:app()
	t.equal(app.sign_in.state, "waiting")
	t.equal(app.sign_in.code, { code = "ABCD-1234", url = "github.com/login/device", expires = "15 minutes" })
	-- Not due yet: no poll.
	r:job("sign_in")
	t.equal(polls, 0)
	r:at("2026-11-10T12:00:05Z"):job("sign_in")
	t.equal(polls, 1)
	-- Pending: the next in five seconds; slow down: ten more.
	r:at("2026-11-10T12:00:10Z"):job("sign_in")
	t.equal(polls, 2)
	r:at("2026-11-10T12:00:15Z"):job("sign_in")
	t.equal(polls, 2)
	r:at("2026-11-10T12:00:19Z"):job("sign_in")
	t.equal(polls, 2)
	r:at("2026-11-10T12:00:20Z"):job("sign_in")
	t.equal(polls, 3)
	t.equal(r.secrets.github, true)
	t.equal(r:app().sign_in, { state = "signed_in", user = "octo" })
	t.equal(asked[#asked], "GET /user")
	-- The token lasts eight hours: then it is forgotten.
	r:at("2026-11-10T20:00:20Z"):job("sign_in")
	t.equal(r.secrets.github, nil)
	t.equal(r:app().sign_in, { state = "signed_out" })
end)

t.case("approving checks at the next tick", function()
	local http = fake({
		["POST /login/device/code"] = DEVICE,
		["POST /login/oauth/access_token"] = { json = { access_token = "gho_token" } },
		["GET /user"] = USER,
	})
	local r = run({ http = http })
	r:action("sign_in", {})
	r:action("check", {})
	r:job("sign_in")
	t.equal(r:app().sign_in, { state = "signed_in", user = "octo" })
end)

t.case("a code that expires, or a refusal on GitHub, says why", function()
	local http = fake({
		["POST /login/device/code"] = DEVICE,
		["POST /login/oauth/access_token"] = { json = { error = "access_denied" } },
	})
	local r = run({ http = http })
	r:action("sign_in", {})
	r:at("2026-11-10T12:00:05Z"):job("sign_in")
	t.equal(r:app().sign_in, { state = "failed", error = "The sign-in was cancelled on GitHub." })
	r = run({ http = fake({
		["POST /login/device/code"] = DEVICE,
		["POST /login/oauth/access_token"] = { json = { error = "authorization_pending" } },
	}) })
	r:action("sign_in", {})
	r:at("2026-11-10T12:15:00Z"):job("sign_in")
	t.equal(r:app().sign_in, { state = "failed", error = "The code expired. Ask for a new one." })
end)

t.case("a disabled device flow is explained", function()
	local http = fake({ ["POST /login/device/code"] = { json = { error = "device_flow_disabled" } } })
	local r = run({ http = http })
	r:action("sign_in", {})
	t.equal(
		r:app().sign_in.error,
		"tau's GitHub App does not allow signing in with a code yet: turn on Device Flow in the app's settings, or use a personal access token."
	)
end)

t.case("a typed token is checked once, and signing out forgets it", function()
	local http, asked = fake({ ["GET /user"] = USER })
	local r = run({ http = http, secrets = { github = true } })
	t.equal(r:app().sign_in, { state = "checking" })
	r:job("sign_in")
	t.equal(r:app().sign_in, { state = "signed_in", user = "octo" })
	r:job("sign_in")
	t.equal(#asked, 1)
	r:action("sign_out", {})
	t.equal(r.secrets.github, nil)
	t.equal(r:app().sign_in, { state = "signed_out" })
end)

t.case("a refused token says why and is forgotten", function()
	local http = fake({ ["GET /user"] = { status = 401, json = { message = "Bad credentials" } } })
	local r = run({ http = http, secrets = { github = true } })
	r:job("sign_in")
	t.equal(r.secrets.github, nil)
	t.equal(r:app().sign_in, { state = "failed", error = "GitHub did not accept the token." })
end)

t.case("the step comes before the repositories and starts a sign-in", function()
	local r = run({ http = fake({}) })
	local step = r:setup(r:app())
	t.equal(step.at, "before_repositories")
	t.equal(step.heading, "Sign in with GitHub")
	t.equal(labels(step.body), { "Sign in with GitHub", "Use a personal access token" })
	t.equal(find(step.body, "button")[1].action, "sign_in")
	-- The token form: its input is a secret's, sent past the plugin.
	r:action("token_form", { open = true })
	local inputs = find(r:setup(r:app()).body, "input")
	t.equal(inputs[1].secret, true)
	t.equal(inputs[1].submit, "tau.secret/github")
end)

t.case("the code opens GitHub, and signed in the step goes on and the section signs out", function()
	local r = run({ http = fake({ ["POST /login/device/code"] = DEVICE, ["GET /user"] = USER }) })
	r:action("sign_in", {})
	local step = r:setup(r:app())
	t.equal(step.heading, "Enter this code on GitHub")
	local open = find(step.body, "button")[1]
	t.equal(open.action, "tau.copy")
	t.equal(open.args, { text = "ABCD-1234", url = "https://github.com/login/device" })
	t.equal(find(step.body, "button")[2].action, "check")
	local signed = run({ http = fake({ ["GET /user"] = USER }), secrets = { github = true } })
	signed:job("sign_in")
	step = signed:setup(signed:app())
	t.equal(step.heading, "Signed in to GitHub")
	t.equal(find(step.body, "button")[1].action, "tau.setup_done")
	local section = signed:models(signed:app())
	t.equal(labels(section), { "Sign out" })
	t.equal(find(section, "text")[1].text, "Signed in as @octo · clones repositories")
end)
