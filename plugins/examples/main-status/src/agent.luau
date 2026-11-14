local heiko = require("heiko")

-- The CI server's status of main, through its MCP server. The server
-- says when it changes; you hear what it reads now.
return heiko.agent {
  triggers = {
    main = { mcp = { server = "ci", uri = "ci://heiko-agent/main" } },
  },
}
