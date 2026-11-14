-- The gate as a machine: every event from every state, in any order,
-- keeps its states in shape, and a change ships only once it was
-- approved and no other was asked since. Then the plugin around it.
local tau = require("tau")
local t = tau.test
local g = t.gen
local gate = require("gate")

local CHANGES = { "v1.2", "v1.3" }

t.machine_of("a change ships only once approved", gate.machine, {
	-- The data each event is drawn with.
	events = {
		ask = g.records({ change = g.sampled_from(CHANGES) }),
		refuse = g.records({ why = g.optional(g.text({ max = 8 })) }),
	},
	-- A model of its own, told each event.
	init = function()
		return { approved = nil }
	end,
	event = function(model, e, state)
		if e.event == "ask" then
			model.approved = nil
		elseif e.event == "approve" and state.state == "approved" then
			model.approved = state.change
		elseif e.event == "shipped" and state.state == "idle" then
			model.approved = nil
		end
		-- A refusal comes only to a change waiting: one approved stays so.
	end,
	invariant = function(state, model)
		for _, change in ipairs(CHANGES) do
			t.equal(gate.allows(state, change), model.approved == change, change)
		end
	end,
})

t.case("a release waits for the person's yes", function()
	local run = t.run({})
	local release = { name = "release", args = { change = "v1.2" } }
	t.equal(run:before_tool(release) ~= nil, true, "not asked")
	run:tool("ask_to_ship", { change = "v1.2" })
	t.equal(run:view().status.text, "waiting to ship v1.2")
	t.equal(run:before_tool(release) ~= nil, true, "not answered")
	run:action("approve", {})
	t.equal(run:before_tool(release), nil, "approved")
	run:after_tool_result({ id = "c1", name = "release", args = release.args, ok = true, text = "shipped" })
	t.equal(run:before_tool(release) ~= nil, true, "shipped once")
end)

-- A click on a stale button: Approve after the person said no has no arm
-- from `refused`, so it changes nothing, and the release stays blocked.
t.case("an answer after a refusal changes nothing", function()
	local run = t.run({})
	run:tool("ask_to_ship", { change = "v1.3" })
	run:action("refuse", { why = "the migration is not ready" })
	t.equal(run.state.gate, { state = "refused", change = "v1.3", why = "the migration is not ready" })
	run:action("approve", {})
	t.equal(run.state.gate.state, "refused")
	t.equal(run:before_tool({ name = "release", args = { change = "v1.3" } }) ~= nil, true)
end)
