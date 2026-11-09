local tau = require("tau")
local t = tau.test

t.case("it wakes on weekday mornings", function()
  local triggers = t.agent({}):describe().triggers
  t.equal(triggers[2].name, "morning")
  t.equal(triggers[2].kind.at, "08:30")
  t.equal(#triggers[2].kind.days, 5)
end)

t.case("the person can ask for a digest", function()
  local triggers = t.agent({}):describe().triggers
  t.equal(triggers[1].kind, { kind = "command", command = "digest", hint = "[what to look at]" })
end)

t.case("it has two triggers", function()
  t.equal(#t.agent({}):describe().triggers, 2)
end)
