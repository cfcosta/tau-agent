local tau = require("tau")
local t = tau.test

t.case("other agents can send it work", function()
  local triggers = t.agent({}):describe().triggers
  t.equal(triggers[1], { name = "inbox", kind = { kind = "message" } })
end)

t.case("the person can send it work too", function()
  local triggers = t.agent({}):describe().triggers
  t.equal(triggers[2].kind, { kind = "command", command = "review", hint = "<what to review>" })
end)

t.case("nothing else wakes it", function()
  t.equal(#t.agent({}):describe().triggers, 2)
end)
