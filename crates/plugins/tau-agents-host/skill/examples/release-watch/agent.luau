local tau = require("tau")
local releases = require("releases")

-- New releases of a project, polled every hour, once each.
return tau.agent {
  name = "release-watch",
  description = "Says what each new release of the project changes for us.",
  triggers = {
    releases = {
      poll = { tool = "mcp__github__list_releases", args = { owner = "cfcosta", repo = "tau-agent" } },
      every = "1h",
      key = releases.key,
    },
    check = { command = "releases" },
  },
}
