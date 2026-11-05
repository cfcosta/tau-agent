local tau = require("tau")
local t = tau.test

t.case("takes the first sentence", function()
	t.equal(t.run({}):title({ task = "fix the login. Then deploy.", model = "m" }), "Fix the login")
end)

t.case("shortens a long one", function()
	local title = t.run({}):title({ task = string.rep("word ", 20), model = "m" })
	t.equal(#title <= 38, true)
	t.equal(string.sub(title, 1, 4), "Word")
end)

t.case("writes none for an empty task", function()
	t.equal(t.run({}):title({ task = "   ", model = "m" }), nil)
end)
