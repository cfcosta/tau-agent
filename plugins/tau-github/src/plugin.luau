-- tau-github: GitHub as where tau's repositories are hosted, through
-- tau's GitHub App (ADR 0045, ADR 0050 gaps E and H). It signs in with a
-- device code or a personal access token, keeps the token as its `github`
-- secret, which its Luau never sees, and fills `tau.forge`: the
-- repositories the app reaches, whether it can push to one, pull
-- requests and their checks. It draws the sign-in as a step of first-run
-- setup and a section of the Models screen.
--
-- The app's store keeps how a sign-in stands: `sign_in`, one going on or
-- failed; `account`, who signed in and when the token expires; and
-- `token_form`, whether the token form shows. A device token lasts eight
-- hours, and renewing one needs the app's secret, so an expired token is
-- forgotten: sign in again.
local tau = require("tau")
local github = require("github")
local endpoints = require("endpoints")
local sign_in = require("sign_in")

local function now(ctx)
	return ctx.now.unix or 0
end

-- How the sign-in stands, as the setup step and the Models screen draw
-- it: `{ state, code?, user?, error? }`.
local function standing(ctx)
	local going = ctx.store.get("sign_in")
	if going then
		return going
	end
	local account = ctx.store.get("account")
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
-- expires: the token form closes, and what failed is gone.
local function signed(ctx, user, expires_in)
	ctx.store.put("account", { user = user, expires_at = expires_in and (now(ctx) + expires_in) or nil })
	ctx.store.delete("sign_in")
	ctx.store.put("token_form", false)
end

local function failed(ctx, error)
	ctx.store.put("sign_in", { state = "failed", error = tostring(error) })
end

-- Who the token belongs to, now it is the secret: signed in, or the token
-- forgotten and why.
local function check_token(ctx, expires_in)
	local ok, user = pcall(github.user, ctx.http)
	if ok then
		signed(ctx, user, expires_in)
	else
		ctx.secrets.forget(github.SECRET)
		ctx.store.delete("account")
		failed(ctx, user)
	end
end

-- The device sign-in's next poll, when it is due.
local function poll(ctx, going)
	local at = now(ctx)
	if at < (going.next or 0) then
		return
	end
	if at >= going.deadline then
		failed(ctx, "The code expired. Ask for a new one.")
		return
	end
	local ok, polled = pcall(github.poll, ctx.http, going.device_code)
	if not ok then
		failed(ctx, polled)
		return
	end
	if polled.pending then
		going.next = at + going.interval
		ctx.store.put("sign_in", going)
	elseif polled.slow_down then
		-- Polling too often: five more seconds each time.
		going.interval += 5
		going.next = at + going.interval
		ctx.store.put("sign_in", going)
	elseif polled.failed then
		failed(ctx, polled.failed)
	else
		check_token(ctx, polled.expires_in)
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
				local going = ctx.store.get("sign_in")
				if going and going.state == "waiting" then
					poll(ctx, going)
				elseif going == nil and account == nil and ctx.secrets.has(github.SECRET) then
					check_token(ctx, nil)
				end
			end,
		},
	},

	actions = {
		-- Asks GitHub for a device code and shows it; the job polls.
		sign_in = function(_, ctx)
			ctx.store.put("token_form", false)
			ctx.store.put("sign_in", { state = "starting" })
			local ok, start = pcall(github.start_device, ctx.http)
			if not ok then
				failed(ctx, start)
				return
			end
			local at = now(ctx)
			local interval = math.max(start.interval, 1)
			ctx.store.put("sign_in", {
				state = "waiting",
				code = {
					code = start.user_code,
					url = github.address(start.verification_uri),
					expires = string.format("%d minutes", start.expires_in // 60),
				},
				device_code = start.device_code,
				deadline = at + start.expires_in,
				interval = interval,
				next = at + interval,
			})
		end,
		-- The person says they approved: poll at the job's next tick.
		check = function(_, ctx)
			local going = ctx.store.get("sign_in")
			if going and going.state == "waiting" then
				going.next = 0
				ctx.store.put("sign_in", going)
			end
		end,
		sign_out = function(_, ctx)
			ctx.secrets.forget(github.SECRET)
			ctx.store.delete("account")
			ctx.store.delete("sign_in")
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
