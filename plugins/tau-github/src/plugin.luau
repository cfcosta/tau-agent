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

local function now(ctx)
	return ctx.now.unix or 0
end

-- How the sign-in stands, as the setup step and the Models screen draw
-- it: `{ state, code?, user?, error? }`.
local function standing(ctx)
	local going = ctx.store.get("sign_in")
	local account = ctx.store.get("account")
	-- A failure stands until a token is typed again: the job checks it.
	local typed = going and going.state == "failed" and account == nil and ctx.secrets.has(github.SECRET)
	if going and going.state ~= "idle" and not typed then
		return going
	end
	if ctx.secrets.has(github.SECRET) then
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
local function signed(ctx, user, expires_in)
	ctx.store.put("account", { user = user, expires_at = expires_in and (now(ctx) + expires_in) or nil })
	ctx.store.put("token_form", false)
end

-- Steps the sign-in with `event` and carries out what it says: a poll of
-- GitHub, or a check of whose the token is, each telling it how it went.
local function step(ctx, event)
	local next, effects = flow.machine:step(ctx.store.get("sign_in"), event)
	ctx.store.put("sign_in", next)
	for _, effect in ipairs(effects) do
		local name = type(effect) == "table" and effect[1] or effect
		if name == "poll" then
			local ok, polled = pcall(github.poll, ctx.http, next.device_code)
			local at = now(ctx)
			if not ok then
				step(ctx, { event = "refused", error = tostring(polled) })
			elseif polled.pending then
				step(ctx, { event = "pending", at = at })
			elseif polled.slow_down then
				step(ctx, { event = "slow_down", at = at })
			elseif polled.failed then
				step(ctx, { event = "refused", error = polled.failed })
			else
				step(ctx, { event = "approved", expires_in = polled.expires_in })
			end
		elseif name == "check_token" then
			local ok, user = pcall(github.user, ctx.http)
			if ok then
				signed(ctx, user, effect[2])
				step(ctx, { event = "checked", ok = true })
			else
				ctx.secrets.forget(github.SECRET)
				ctx.store.delete("account")
				step(ctx, { event = "checked", ok = false, error = tostring(user) })
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
			run = function(ctx)
				local account = ctx.store.get("account")
				if account and account.expires_at and now(ctx) >= account.expires_at then
					ctx.secrets.forget(github.SECRET)
					ctx.store.delete("account")
					return
				end
				if flow.machine:is(ctx.store.get("sign_in"), "waiting") then
					step(ctx, { event = "tick", at = now(ctx) })
				elseif account == nil and ctx.secrets.has(github.SECRET) then
					step(ctx, { event = "token" })
				end
			end,
		},
	},

	actions = {
		-- Asks GitHub for a device code and shows it; the job polls.
		sign_in = function(_, ctx)
			ctx.store.put("token_form", false)
			step(ctx, { event = "start" })
			local ok, start = pcall(github.start_device, ctx.http)
			if not ok then
				step(ctx, { event = "start_failed", error = tostring(start) })
				return
			end
			step(ctx, {
				event = "started",
				at = now(ctx),
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
		check = function(_, ctx)
			step(ctx, { event = "approve" })
		end,
		sign_out = function(_, ctx)
			ctx.secrets.forget(github.SECRET)
			ctx.store.delete("account")
			step(ctx, { event = "sign_out" })
		end,
		-- Shows the token form, or the device code again.
		token_form = function(args, ctx)
			ctx.store.put("token_form", args.open == true)
		end,
	},

	app = function(ctx)
		return { sign_in = standing(ctx), token_form = ctx.store.get("token_form") == true }
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
		account = function(_, ctx)
			local account = ctx.store.get("account")
			if account and ctx.secrets.has(github.SECRET) then
				return account.user
			end
			return json.null
		end,
		repos = function(_, ctx)
			return array(github.repos(ctx.http))
		end,
		can_push = function(input, ctx)
			return github.can_push(ctx.http, input.repo)
		end,
		open_pull = function(input, ctx)
			return github.open_pull(ctx.http, input)
		end,
		checks = function(input, ctx)
			return github.checks(ctx.http, input.repo, input.head)
		end,
	},
})
