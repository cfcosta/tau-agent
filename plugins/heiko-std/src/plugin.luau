-- heiko-std is not a plugin heiko runs: its files are the parts of the
-- `heiko` module every plugin has (`heiko.text`, `heiko.count`,
-- `heiko.values`, `heiko.store`, `heiko.jev`, `heiko.infer`, `heiko.now`),
-- which heiko installs itself. This entry is here so the package's tests run as any
-- package's do, with a store, Jev and the model to reach.
local heiko = require("heiko")

return heiko.plugin({
	uses = { store = true, jev = true, infer = true },
})
