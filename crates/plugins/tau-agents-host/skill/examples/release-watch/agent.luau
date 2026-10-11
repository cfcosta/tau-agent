local tau = require("tau")
local releases = require("releases")

-- New releases of a project, polled every hour through its MCP server,
-- and its tags straight from GitHub's API, each once.
return tau.agent {
  name = "release-watch",
  description = "Says what each new release of the project changes for us.",
  triggers = {
    releases = {
      poll = { tool = "mcp__github__list_releases", args = { owner = "cfcosta", repo = "tau-agent" } },
      every = "1h",
      key = releases.key,
      -- Drafts are not releases yet.
      items = releases.published,
    },
    tags = {
      poll = {
        http = {
          url = "https://api.github.com/repos/cfcosta/tau-agent/tags",
          headers = {
            -- A key from tau's secret store, by name: tau puts it in.
            authorization = "Bearer " .. tau.secret("github"),
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
