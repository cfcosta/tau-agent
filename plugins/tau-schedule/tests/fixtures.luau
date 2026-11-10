-- The demo's chat (ADR 0053): it scheduled two looks ahead, and woke on
-- a third; the fixture is what the plugin leaves and draws of it.
local tau = require("tau")
local t = tau.test

-- When the demo's fixture was made: 2026-10-12 12:00 UTC. The demo
-- moves each time in it by how far its own now is from this.
local DEMO_NOW = "2026-10-12T12:00:00Z"

-- What the demo's chat scheduled: name, how far ahead, note.
local DEMO_WAKES = {
	{
		"recheck-415",
		"2h",
		"If #415 has its second approval, land it; if CI is red, read the "
			.. "failing job before touching anything.",
	},
	{
		"release-notes",
		"26h",
		"Tomorrow's release: check the notes mention the plugins tree change from #412.",
	},
}

-- The message the demo's chat woke on.
local DEMO_WOKEN = "[scheduled · look-at-415 · 2026-10-12 14:00]\n"
	.. "Read #415's review. Answer what you can; if a change is asked for, make it and push."

t.fixture("scheduled", function()
	local r = t.run({ id = "scheduled-review", kind = "chat", now = DEMO_NOW, offset = 0 })
	for _, w in DEMO_WAKES do
		r:tool("schedule", { name = w[1], note = w[3], ["in"] = w[2] })
	end
	r:start({ input = DEMO_WOKEN })
	local view = r:view()
	return { now = r.data.now.unix, state = r.state, view = view }
end)
