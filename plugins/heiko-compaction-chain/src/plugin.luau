-- The chain of compactions (ADR 0045, 0050): it fills `heiko.compaction`
-- with fast pruning, the tree and the summary, which rewrite in that
-- turn, each declining when it would save too little.
local heiko = require("heiko")

return heiko.plugin({
	fills = { "heiko.compaction" },
	compaction = {
		rewriters = { "heiko-fast-compaction", "heiko-tree-compaction", "heiko-compaction" },
	},
})
