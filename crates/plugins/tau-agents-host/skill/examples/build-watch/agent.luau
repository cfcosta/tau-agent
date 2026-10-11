local tau = require("tau")

-- The CI's events as they happen; only failed builds are events.
return tau.agent {
  name = "build-watch",
  description = "Looks into each failed build of main and says why it failed.",
  triggers = {
    builds = {
      stream = { sse = "https://ci.example.com/v1/pipelines/tau/events", headers = { authorization = tau.secret("ci") } },
      parse = function(frame, state)
        if frame.event ~= "build" then
          return nil
        end
        local build = json.decode(frame.data)
        if build.state ~= "failed" then
          return nil
        end
        return { id = "build:" .. build.number, data = build }
      end,
    },
  },
}
