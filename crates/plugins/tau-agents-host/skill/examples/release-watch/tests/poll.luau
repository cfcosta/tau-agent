local tau = require("tau")
local t = tau.test

t.case("the first poll only records what is there", function()
  local agent = t.agent {}
  t.equal(#agent:poll("releases", { { tag_name = "v1" }, { tag_name = "v2" } }), 0)
end)

t.case("a release it has not seen is an event", function()
  local agent = t.agent { state = { releases = { seen = { "v1" } } } }
  local events = agent:poll("releases", { { tag_name = "v1" }, { tag_name = "v2" } })
  t.equal(#events, 1)
  t.equal(events[1].id, "releases:v2")
end)

t.case("a release seen twice is one event", function()
  local agent = t.agent { state = { releases = { seen = {} } } }
  t.equal(#agent:poll("releases", { { tag_name = "v3" } }), 1)
  t.equal(#agent:poll("releases", { { tag_name = "v3" } }), 0)
end)
