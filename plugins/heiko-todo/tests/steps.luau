local heiko = require("heiko")
local t = heiko.test

t.case("keeps the steps it is given, replaced whole", function()
	local run = t.run({})
	run:tool("todo_steps", { steps = { { text = "read", status = "done" }, { text = "fix", status = "in_progress" } } })
	run:tool("todo_steps", { steps = { { text = "test", status = "pending" } } })
	t.equal(run.state.steps, { { text = "test", status = "pending" } })
end)

t.case("refuses two steps in progress", function()
	local ok = pcall(function()
		t.run({}):tool("todo_steps", { steps = { { text = "a", status = "in_progress" }, { text = "b", status = "in_progress" } } })
	end)
	t.equal(ok, false)
end)

t.case("its band says the step it is on and which of how many", function()
	local run = t.run({ state = { steps = { { text = "read", status = "done" }, { text = "fix", status = "in_progress" } } } })
	local lead = run:view().band.lead
	t.equal({ lead[2].text, lead[3].text }, { "2 of 2", "fix" })
end)
