local heiko = require("heiko")
local releases = require("releases")

-- New releases of a project, polled every hour through its MCP server,
-- and its tags straight from GitHub's API, each once.
return heiko.agent {
  triggers = {
    releases = {
      poll = { tool = "mcp__github__list_releases", args = { owner = "cfcosta", repo = "heiko-agent" } },
      every = "1h",
      key = releases.key,
      -- Drafts are not releases yet.
      items = releases.published,
    },
    tags = {
      poll = {
        http = {
          url = "https://api.github.com/repos/cfcosta/heiko-agent/tags",
          headers = {
            -- A key from heiko's secret store, by name: heiko puts it in.
            authorization = "Bearer " .. heiko.secret("github"),
            accept = "application/vnd.github+json",
          },
        },
      },
      every = "6h",
      key = "name",
    },
    check = { command = "releases" },
  },
}
