local tau = require("tau")
local t = tau.test

t.case("fills tau.compaction with the three, in their turn", function()
	local plugin = t.load()
	t.equal(plugin.fills, { "tau.compaction" })
	t.equal(plugin.compaction.rewriters, { "tau-fast-compaction", "tau-tree-compaction", "tau-compaction" })
end)
