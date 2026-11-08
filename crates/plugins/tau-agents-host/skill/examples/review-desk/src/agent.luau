local tau = require("tau")

-- Reviews what other agents send it, and what the person asks for with
-- /review. What it finds goes to the person with notify, or back to the
-- agent that asked with agent_send.
return tau.agent {
  triggers = {
    inbox = { message = true },
    review = { command = "review", hint = "<what to review>" },
  },
}
