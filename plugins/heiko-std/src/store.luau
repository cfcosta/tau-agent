-- `heiko.store`: the plugin's store in the repository, awaited (ADR
-- 0056). A read waits for its answer; a write is given, and lands when
-- the step ends, before the next read. Only a hook may call these, in a
-- plugin whose `uses` has `store`.

return function(heiko)
-- The store `commands` make commands for, as calls.
local function awaited(commands)
	local store = {}

	-- The value of `key`, or nil.
	function store.get(key)
		return heiko.await(commands.get(key))
	end

	-- Every `{ key, value }` whose key starts with `prefix`, in the keys'
	-- order.
	function store.list(prefix)
		return heiko.await(commands.list(prefix))
	end

	-- Sets `key` to `value`.
	function store.put(key, value)
		heiko.give(commands.put(key, value))
	end

	-- Deletes `key`.
	function store.delete(key)
		heiko.give(commands.delete(key))
	end

	-- Adds one to `rev`, the count a repository's pages and its sidebar
	-- entry draw again on (`repo_data` gives it back as `rev`).
	function store.bump()
		store.put("rev", (store.get("rev") or 0) + 1)
	end

	return store
end

local store = awaited(heiko.cmd.store)
heiko.store = store

-- The same calls on the plugin's store in the repository `repo`: for a
-- job, which is in no repository, in a plugin whose `uses` has `chats`.
function store.of(repo)
	return awaited(heiko.cmd.stores(repo))
end

end
