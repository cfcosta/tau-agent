local tau = require("tau")
local t = tau.test

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

t.case("its band says the next step and how many are done", function()
	local run = t.run({ state = { steps = { { text = "read", status = "done" }, { text = "fix", status = "in_progress" } } } })
	t.equal(run:view().band.title, "Next: fix, 1/2")
end)
