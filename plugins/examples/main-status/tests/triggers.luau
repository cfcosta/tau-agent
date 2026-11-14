local heiko = require("heiko")
local t = heiko.test

t.case("it watches main's status on the ci server", function()
  local triggers = t.agent({}):describe().triggers
  t.equal(triggers[1], {
    name = "main",
    kind = { kind = "mcp", server = "ci", uri = "ci://heiko-agent/main" },
  })
end)

t.case("nothing else wakes it", function()
  t.equal(#t.agent({}):describe().triggers, 1)
end)

t.case("it needs no key: the server is the person's", function()
  t.equal(#t.agent({}):describe().secrets, 0)
end)
