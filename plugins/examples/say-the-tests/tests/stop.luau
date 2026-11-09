local tau = require("tau")
local ui = tau.ui
local t = tau.test

local function edited()
	local run = t.run({})
	run:turn_end({ turn = 1, text = "", calls = { { name = "edit", ok = true } } })
	return run
end

t.case("a run that changed nothing stops", function()
	local run = t.run({})
	run:turn_end({ turn = 1, text = "", calls = { { name = "read", ok = true } } })
	t.equal(run:before_stop({ text = "Done.", turn = 1, reason = "stop" }), nil)
end)

t.case("a run that changed files says its tests", function()
	local run = edited()
	t.equal(
		run:before_stop({ text = "Done.", turn = 2, reason = "stop" }),
		tau.continue("You changed files: say which tests you ran, and how they went.")
	)
	t.equal(run.logs, { "asked for the tests on turn 2" })
	t.equal(run:before_stop({ text = "Tests pass.", turn = 3, reason = "stop" }), nil)
end)

t.case("a failed edit changed nothing", function()
	local run = t.run({})
	run:turn_end({ turn = 1, text = "", calls = { { name = "write", ok = false } } })
	t.equal(run.state, {})
end)

t.case("the status line says it is watching", function()
	t.equal(edited():view(), { status = ui.status("watching", "files changed", "info") })
end)
