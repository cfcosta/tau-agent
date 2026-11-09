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

t.case("a draft is not an event until it is out", function()
  local agent = t.agent { state = { releases = { seen = {} } } }
  t.equal(#agent:poll("releases", { { tag_name = "v4", draft = true } }), 0)
  t.equal(#agent:poll("releases", { { tag_name = "v4", draft = false } }), 1)
end)

t.case("a new tag is an event, keyed by its name", function()
  local agent = t.agent { state = { tags = { seen = { "v1" } } } }
  local events = agent:poll("tags", { { name = "v1" }, { name = "v2" } })
  t.equal(#events, 1)
  t.equal(events[1].id, "tags:v2")
end)

t.case("the request names its key, not its value", function()
  local described = t.agent {}:describe()
  local found
  for _, trigger in ipairs(described.triggers) do
    if trigger.name == "tags" then
      found = trigger.kind
    end
  end
  t.equal(found.http.headers.authorization, { ["$secret"] = "github", prefix = "Bearer " })
  t.equal(described.secrets, { "github" })
end)
