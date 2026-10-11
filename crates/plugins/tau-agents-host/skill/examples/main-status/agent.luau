local tau = require("tau")

-- The CI server's status of main, through its MCP server. The server
-- says when it changes; you hear what it reads now.
return tau.agent {
  name = "main-status",
  description = "Says when main's build turns red or green, and why.",
  triggers = {
    main = { mcp = { server = "ci", uri = "ci://tau-agent/main" } },
  },
}
