local heiko = require("heiko")
local t = heiko.test

t.case("fills heiko.compaction with the three, in their turn", function()
	local plugin = t.load()
	t.equal(plugin.fills, { "heiko.compaction" })
	t.equal(plugin.compaction.rewriters, { "heiko-fast-compaction", "heiko-tree-compaction", "heiko-compaction" })
end)
