local tau = require("tau")
local t = tau.test

t.case("a keep-alive is not an event", function()
  t.equal(t.agent({}):parse("builds", { event = "ping", data = "" }), nil)
end)

t.case("a passing build is an event too: the agent judges, not parse", function()
  local frame = { event = "build", data = '{"number":7,"state":"passed"}' }
  local event = t.agent({}):parse("builds", frame)
  t.equal(event.id, "build:7")
  t.equal(event.data.state, "passed")
end)

t.case("a failed build is an event keyed by its number", function()
  local frame = { event = "build", data = '{"number":8,"state":"failed"}' }
  local event = t.agent({}):parse("builds", frame)
  t.equal(event.id, "build:8")
  t.equal(event.data.state, "failed")
end)
