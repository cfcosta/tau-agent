-- tau-github: GitHub as where tau's repositories are hosted, through
-- tau's GitHub App (ADR 0045, ADR 0050 gaps E and H). It signs in with a
-- device code or a personal access token, keeps the token as its `github`
-- secret, which its Luau never sees, and fills `tau.forge`: the
-- repositories the app reaches, whether it can push to one, pull
-- requests and their checks. It draws the sign-in as a step of first-run
-- setup and a section of the Models screen.
--
-- The app's store keeps how a sign-in stands: `sign_in`, a state of
-- `flow.machine`; `account`, who signed in and when the token expires;
-- and `token_form`, whether the token form shows. A device token lasts eight
-- hours, and renewing one needs the app's secret, so an expired token is
-- forgotten: sign in again.
local tau = require("tau")
local github = require("github")
local endpoints = require("endpoints")
local sign_in = require("sign_in")
local flow = require("flow")

local cmd = tau.cmd

local function get(key)
	return tau.await(cmd.store.get(key))
end

local function has_token()
	return tau.await(cmd.secrets.has(github.SECRET))
end

local function now(env)
	return env.now.unix or 0
end

-- How the sign-in stands, as the setup step and the Models screen draw
-- it: `{ state, code?, user?, error? }`.
local function standing()
	local going = get("sign_in")
	local account = get("account")
	local token = has_token()
	-- A failure stands until a token is typed again: the job checks it.
	local typed = going and going.state == "failed" and account == nil and token
	if going and going.state ~= "idle" and not typed then
		return going
	end
	if token then
		if account then
			return { state = "signed_in", user = account.user }
		end
		-- A token typed in, not checked yet: the job checks it.
		return { state = "checking" }
	end
	return { state = "signed_out" }
end

-- Signed in as `user`, with a token that lasts `expires_in` seconds if it
-- expires: the token form closes.
local function signed(env, user, expires_in)
	tau.give(cmd.store.put("account", { user = user, expires_at = expires_in and (now(env) + expires_in) or nil }))
	tau.give(cmd.store.put("token_form", false))
end

-- Steps the sign-in with `event` and carries out what it says: a poll of
-- GitHub, or a check of whose the token is, each telling it how it went.
-- The sign-in is read from the store at each step, so one stepped while
-- a request was out is what the answer steps on.
local function step(env, event)
	local next, effects = flow.machine:step(get("sign_in"), event)
	tau.give(cmd.store.put("sign_in", next))
	for _, effect in ipairs(effects) do
		local name = type(effect) == "table" and effect[1] or effect
		if name == "poll" then
			local ok, polled = pcall(github.poll, next.device_code)
			local at = now(env)
			if not ok then
				step(env, { event = "refused", error = tostring(polled) })
			elseif polled.pending then
				step(env, { event = "pending", at = at })
			elseif polled.slow_down then
				step(env, { event = "slow_down", at = at })
			elseif polled.failed then
				step(env, { event = "refused", error = polled.failed })
			else
				step(env, { event = "approved", expires_in = polled.expires_in })
			end
		elseif name == "check_token" then
			local ok, user = pcall(github.user)
			if ok then
				signed(env, user, effect[2])
				step(env, { event = "checked", ok = true })
			else
				tau.give(cmd.secrets.forget(github.SECRET))
				tau.give(cmd.store.delete("account"))
				step(env, { event = "checked", ok = false, error = tostring(user) })
			end
		end
	end
end

return tau.plugin({
	uses = {
		network = endpoints.hosts,
		secrets = { github.SECRET },
		store = true,
	},
	fills = { "tau.forge" },

	-- The sign-in's background: a device code polled at GitHub's interval
	-- until the person approves; a typed token checked once; an expired one
	-- forgotten. It does nothing while none of these is so.
	jobs = {
		sign_in = {
			every = 1,
			run = function(_, _, env)
				local account = get("account")
				if account and account.expires_at and now(env) >= account.expires_at then
					tau.give(cmd.secrets.forget(github.SECRET))
					tau.give(cmd.store.delete("account"))
					return
				end
				if flow.machine:is(get("sign_in"), "waiting") then
					step(env, { event = "tick", at = now(env) })
				elseif has_token() and get("account") == nil then
					step(env, { event = "token" })
				end
			end,
		},
	},

	actions = {
		-- Asks GitHub for a device code and shows it; the job polls.
		sign_in = function(_, _, env)
			tau.give(cmd.store.put("token_form", false))
			step(env, { event = "start" })
			local ok, start = pcall(github.start_device)
			if not ok then
				step(env, { event = "start_failed", error = tostring(start) })
				return
			end
			step(env, {
				event = "started",
				at = now(env),
				code = {
					code = start.user_code,
					url = github.address(start.verification_uri),
					expires = string.format("%d minutes", start.expires_in // 60),
				},
				device_code = start.device_code,
				expires_in = start.expires_in,
				interval = start.interval,
			})
		end,
		-- The person says they approved: poll at the job's next tick.
		check = function(_, _, env)
			step(env, { event = "approve" })
		end,
		sign_out = function(_, _, env)
			tau.give(cmd.secrets.forget(github.SECRET))
			tau.give(cmd.store.delete("account"))
			step(env, { event = "sign_out" })
		end,
		-- Shows the token form, or the device code again.
		token_form = function(args)
			tau.give(cmd.store.put("token_form", args.open == true))
		end,
	},

	app = function()
		return { sign_in = standing(), token_form = get("token_form") == true }
	end,
	setup = function(app)
		return sign_in.step(app.sign_in, app.token_form)
	end,
	models = function(app)
		return sign_in.section(app.sign_in, app.token_form)
	end,

	forge = {
		name = "GitHub",
		id = "github",
		host = "github.com",
		token = github.SECRET,
		-- Who is signed in: none with no token, or one not checked yet.
		account = function()
			if not has_token() then
				return json.null
			end
			-- Read after the secret's answer: a sign-out may have come.
			local account = get("account")
			if account then
				return account.user
			end
			return json.null
		end,
		repos = function()
			return array(github.repos())
		end,
		can_push = function(input)
			return github.can_push(input.repo)
		end,
		open_pull = function(input)
			return github.open_pull(input)
		end,
		checks = function(input)
			return github.checks(input.repo, input.head)
		end,
	},
})
