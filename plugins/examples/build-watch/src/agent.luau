local heiko = require("heiko")

-- The CI's events as they happen. `parse` only reads frames: a
-- keep-alive is not an event, every build is, and the agent decides in
-- its conversation which ones need looking into.
return heiko.agent {
  triggers = {
    builds = {
      stream = { sse = "https://ci.example.com/v1/pipelines/heiko/events", headers = { authorization = heiko.secret("ci") } },
      parse = function(frame, state)
        if frame.event ~= "build" then
          return nil
        end
        local build = json.decode(frame.data)
        return { id = "build:" .. build.number, data = build }
      end,
    },
  },
}
