local tau = require("tau")

-- Each weekday morning, and whenever the person asks with /digest.
return tau.agent {
  name = "morning-digest",
  description = "Each weekday morning, what landed yesterday and what is still open.",
  triggers = {
    morning = { at = "08:30", days = { "Mon", "Tue", "Wed", "Thu", "Fri" } },
    digest = { command = "digest", hint = "[what to look at]" },
  },
}
