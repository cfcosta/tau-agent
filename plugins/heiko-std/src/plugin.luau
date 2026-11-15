-- heiko-std is not a plugin heiko runs: its files are the parts of the
-- `heiko` module every plugin has (`heiko.text`, `heiko.count`,
-- `heiko.values`, `heiko.store`, `heiko.decide`, `heiko.infer`, `heiko.now`),
-- which heiko installs itself. This entry is here so the package's tests run as any
-- package's do, with a store, the decider and the model to reach.
local heiko = require("heiko")

return heiko.plugin({
	uses = { store = true, decide = true, infer = true },
})
