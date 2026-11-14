local heiko = require("heiko")
local t = heiko.test

t.case("shows each notice on the desktop", function()
	local run = t.run({})
	run:notify({ title = "heiko-agent is ready to land", body = "retry-after" })
	run:notify({ title = "A chat asks you something", body = "" })
	t.equal(run.notices, {
		{ title = "heiko-agent is ready to land", body = "retry-after" },
		{ title = "A chat asks you something", body = "" },
	})
end)

t.case("shows none when switched off", function()
	local run = t.run({ settings = { on = false } })
	run:notify({ title = "heiko-agent is ready to land", body = "retry-after" })
	t.equal(#run.notices, 0)
end)

t.case("is on until switched off", function()
	local run = t.run({})
	run:notify({ title = "A run failed", body = "the model refused" })
	t.equal(#run.notices, 1)
end)
