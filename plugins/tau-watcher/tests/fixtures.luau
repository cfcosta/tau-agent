-- The views the demo shows: what the plugin draws from a note kept at
-- the 12th step, new, learned about and already known.
local tau = require("tau")
local t = tau.test

local LINE = "The agent now counts queued forks in the “N need you” pill, so the pill can show a number when nothing needs a decision."

-- The watcher's state with its one note at `status`.
local function state_with(status)
	return {
		notes = {
			{
				key = "n0",
				step = 12,
				tag = "Heads up",
				line = LINE,
				explain = {
					title = "Queued chats now raise the pill",
					bullets = {
						"A fork waiting in the queue counts as needing you, even though it asks for no decision.",
						"The pill can read “2 need you” while every chat is only waiting to start.",
						"Running a queued chat, or removing it, lowers the count again.",
						"The change is in the sidebar's attention states, not in the chats themselves.",
					},
				},
				typed_past = 0,
				status = status,
			},
		},
		ignored = 0,
		dropped = 0,
		last_check = 12,
	}
end

-- The state and what the plugin's view draws of it.
local function drawn(status)
	local state = state_with(status)
	local run = t.run({ id = "demo", settings = { enabled = true, family = "chat" }, state = state_with(status) })
	return { state = state, view = run:view() }
end

for _, status in ipairs({ "new", "learned", "knew" }) do
	t.fixture(status, function()
		return drawn(status)
	end)
end
