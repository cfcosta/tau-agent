local tau = require("tau")
local t = tau.test

t.case("a keep-alive is not an event", function()
  t.equal(t.agent({}):parse("builds", { event = "ping", data = "" }), nil)
end)

t.case("a passing build is not an event", function()
  local frame = { event = "build", data = '{"number":7,"state":"passed"}' }
  t.equal(t.agent({}):parse("builds", frame), nil)
end)

t.case("a failed build is an event keyed by its number", function()
  local frame = { event = "build", data = '{"number":8,"state":"failed"}' }
  local event = t.agent({}):parse("builds", frame)
  t.equal(event.id, "build:8")
  t.equal(event.data.state, "failed")
end)
