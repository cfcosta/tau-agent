local heiko = require("heiko")

-- Each weekday morning, and whenever the person asks with /digest.
return heiko.agent {
  triggers = {
    morning = { at = "08:30", days = { "Mon", "Tue", "Wed", "Thu", "Fri" } },
    digest = { command = "digest", hint = "[what to look at]" },
  },
}
