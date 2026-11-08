-- The chain of compactions (ADR 0045, 0050): it fills `tau.compaction`
-- with fast pruning, the tree and the summary, which rewrite in that
-- turn, each declining when it would save too little.
local tau = require("tau")

return tau.plugin({
	fills = { "tau.compaction" },
	compaction = {
		rewriters = { "tau-fast-compaction", "tau-tree-compaction", "tau-compaction" },
	},
})
