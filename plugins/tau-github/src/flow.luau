-- How a sign-in to GitHub goes (ADR 0054), as the app's store keeps it
-- under `sign_in`: none going on, a device code being asked for, one
-- waiting for the person to approve it, or one that failed. Who signed in
-- is `account`, apart: a token checked, by a device sign-in or typed in
-- the form, makes it.
local tau = require("tau")

local flow = {}

local function starting()
	return { state = "starting" }
end

-- A token is there and no one is signed in: typed in the form, which
-- sets the secret past the plugin. It is checked from a failure too, so
-- a token typed after a refused one is not left unchecked.
local function check()
	return { state = "idle" }, { "check_token" }
end

local function gone()
	return { state = "idle" }
end

flow.machine = tau.machine({
	states = {
		idle = {},
		starting = {},
		waiting = { "code", "device_code", "deadline", "interval", "next" },
		failed = { "error" },
	},
	initial = { state = "idle" },
	effects = { "poll", "check_token" },
	on = {
		start = { idle = starting, failed = starting, waiting = starting },
		started = {
			starting = function(_, e)
				local interval = math.max(e.interval, 1)
				return {
					state = "waiting",
					code = e.code,
					device_code = e.device_code,
					deadline = e.at + e.expires_in,
					interval = interval,
					next = e.at + interval,
				}
			end,
		},
		start_failed = {
			starting = function(_, e)
				return { state = "failed", error = e.error }
			end,
		},
		-- The job's tick: a poll when one is due, the code given up once
		-- it expired.
		tick = {
			waiting = function(s, e)
				if e.at >= s.deadline then
					return { state = "failed", error = "The code expired. Ask for a new one." }
				end
				if e.at < s.next then
					return s
				end
				return s, { "poll" }
			end,
		},
		-- The person says they approved: poll at the next tick.
		approve = {
			waiting = function(s)
				local next = table.clone(s)
				next.next = 0
				return next
			end,
		},
		pending = {
			waiting = function(s, e)
				local next = table.clone(s)
				next.next = e.at + s.interval
				return next
			end,
		},
		-- Polling too often: five more seconds each time.
		slow_down = {
			waiting = function(s, e)
				local next = table.clone(s)
				next.interval = s.interval + 5
				next.next = e.at + next.interval
				return next
			end,
		},
		approved = {
			waiting = function(_, e)
				return { state = "idle" }, { { "check_token", e.expires_in } }
			end,
		},
		refused = {
			waiting = function(_, e)
				return { state = "failed", error = e.error }
			end,
		},
		token = { idle = check, failed = check },
		checked = {
			idle = function(s, e)
				if e.ok then
					return s
				end
				return { state = "failed", error = e.error }
			end,
		},
		sign_out = { idle = gone, starting = gone, waiting = gone, failed = gone },
	},
})

return flow
