local tau = require("tau")
local t = tau.test

local FRIDAY = "2026-10-09T10:00:00Z"
local MONDAY = "2026-10-05T10:00:00Z"

local function deploy(run)
	return run:before_tool({ name = "bash", args = { command = "make deploy" } })
end

t.case("blocks deploys on Friday", function()
	t.equal(deploy(t.run({ now = FRIDAY })), tau.block("No deploys on Friday."))
end)

t.case("allows them on Monday", function()
	t.equal(deploy(t.run({ now = MONDAY })), nil)
end)

t.case("follows the days the person set", function()
	local run = t.run({ now = MONDAY, settings = { days = { "Monday" } } })
	t.equal(deploy(run), tau.block("No deploys on Monday."))
end)

t.case("leaves other commands alone", function()
	local run = t.run({ now = FRIDAY })
	t.equal(run:before_tool({ name = "bash", args = { command = "make test" } }), nil)
	t.equal(run:before_tool({ name = "read", args = { path = "deploy.md" } }), nil)
end)
