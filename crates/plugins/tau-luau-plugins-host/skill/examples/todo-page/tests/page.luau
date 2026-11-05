local tau = require("tau")
local t = tau.test

t.case("adds what the person types", function()
	local run = t.run({})
	run:action("add", { text = "write the notes" })
	t.equal(run.state.items, { { text = "write the notes", done = false } })
end)

t.case("marks one done", function()
	local run = t.run({ state = { items = { { text = "a", done = false } } } })
	run:action("done", { at = 1 })
	t.equal(run.state.items[1].done, true)
end)

t.case("its row says how many are left", function()
	local run = t.run({ state = { items = { { text = "a", done = false }, { text = "b", done = true } } } })
	t.equal(run:view().row, { line = "1 to do" })
	t.equal(t.run({}):view().row, nil)
end)
