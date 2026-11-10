-- One scope's memory: its notes and the index that finds them, kept in
-- step. Every write goes through here, so every write is checked
-- (`safety`), indexed, and answered with the notes nearest to it, for
-- the agent to link or supersede instead of repeating.
--
-- What the rest of the plugin, its imports too, use:
--
--   memory.open(files, dir)            a scope: the notes in `dir`, read
--                                      through `files` (`ctx.files`)
--   memory.scopes(ctx)                 the run's scopes, `{ repository,
--                                      user }`; either may be missing
--   memory.write(scopes, name, draft, now)
--                                      writes `draft` to `scopes[name]`:
--                                      `written`, or nil, why and `kind`
--                                      ("refused" or "store")
--   memory.search(scopes, query, limit)
--                                      hits across both scopes, the
--                                      user's ids `user:<id>`
--   scope:write(draft, now)            as `memory.write`, on one scope
--   scope:link(from, to, type, why, now), scope:search(query, budget),
--   scope:read(id), scope:mark_stale(paths, why, written_by, now),
--   scope:index_note(), scope.notes (the store)
--   memory.note                        the note module: parse, render,
--                                      validate, slug, is_id, all_links
--
-- A draft is `{ type, title, description, body, tags, links, id?,
-- supersedes?, source }`, `links` each `{ to, type, why? }` and
-- `source` `{ by, run?, turn?, commit?, files, imported? }`. What a write
-- answers is `{ id, action ("created", "updated" or "superseded"),
-- superseded (the id it replaced), redacted (how many secrets went),
-- nearest (up to three hits, itself left out) }`.
local bm25 = require("bm25")
local note = require("note")
local recall = require("recall")
local safety = require("safety")
local store = require("store")
local text = require("text")

local M = {}

M.note = note

-- The id of a scope's index note: the map loaded into every run.
M.INDEX_ID = "index"

-- The index note's size limit, in characters: about 2,000 tokens.
M.INDEX_BUDGET = 8000

-- How many near notes a write reports.
M.NEAREST = 3

-- The prefix of ids from the user's scope.
M.USER = "user:"

-- Where the plugin keeps notes, in its own folders: the repository's,
-- and the user's across repositories.
M.FOLDER = "tau-memory"

local Memory = {}
Memory.__index = Memory

-- Reads the notes in `dir` and indexes them all: the index is derived,
-- and rebuilt from the files on every open.
function M.open(files, dir)
	local notes = store.open(files, dir)
	local index = bm25.new()
	for _, n in ipairs(notes:list()) do
		index:upsert(n.id, bm25.index_text(n))
	end
	return setmetatable({ notes = notes, index = index, dir = dir }, Memory)
end

-- The scope's index note, when one was written.
function Memory:index_note()
	return self.notes:get(M.INDEX_ID)
end

local function refused(why)
	return nil, why, "refused"
end

-- What a failed store operation answers.
local function failed(err)
	if err.kind == "note" then
		return nil, err.message, "refused"
	end
	return nil, err.message, "store"
end

-- Checks, redacts and stores `draft`, then indexes it.
function Memory:write(draft, now)
	local tags = draft.tags or {}
	local links = draft.links or {}
	local checked = { draft.title, draft.description, draft.body }
	for _, tag in ipairs(tags) do
		table.insert(checked, tag)
	end
	for _, link in ipairs(links) do
		if link.why ~= nil then
			table.insert(checked, link.why)
		end
	end
	for _, s in ipairs(checked) do
		local why = safety.refusal(s)
		if why then
			return refused("not stored: " .. why)
		end
	end
	local redacted = 0
	local function clean(s)
		local out, count = safety.redact(s)
		redacted += count
		return out
	end
	local title, description, body = clean(draft.title), clean(draft.description), clean(draft.body)
	local clean_tags = {}
	for _, tag in ipairs(tags) do
		table.insert(clean_tags, clean(tag))
	end
	local clean_links = {}
	for _, link in ipairs(links) do
		table.insert(clean_links, { to = link.to, type = link.type, why = link.why ~= nil and clean(link.why) or nil })
	end

	local kind = draft.type
	local named = draft.id ~= nil
	local target = draft.id
	if target == nil then
		target = kind == "index" and M.INDEX_ID or note.slug(title)
	end
	-- A replacement often keeps the old title; it takes the next free id.
	if draft.supersedes ~= nil and not named then
		local base = target
		local n = 2
		while self.notes:get(target) do
			target = string.sub(base, 1, math.min(#base, 60)) .. "-" .. n
			n += 1
		end
	end
	if not note.is_id(target) then
		return refused(store.quoted(target) .. " is not a note id: use a-z, 0-9 and -")
	end
	if (kind == "index") ~= (target == M.INDEX_ID) then
		return refused('the index note is the only note of type index, and is called "' .. M.INDEX_ID .. '"')
	end
	if kind == "index" and text.length(body) > M.INDEX_BUDGET then
		return refused(
			"the index note would be "
				.. text.length(body)
				.. " characters, over its "
				.. M.INDEX_BUDGET
				.. "; rewrite it shorter: one line per note or group"
		)
	end

	local existing = self.notes:get(target)
	-- A title that lands on another note's id never overwrites it: the
	-- agent names the note it means to change. The index note has one
	-- id, so it is always meant.
	if existing and not named and draft.supersedes == nil and kind ~= "index" then
		local q = store.quoted(target)
		return refused(
			"a note called " .. q .. " exists; pass id " .. q .. " to update it, supersedes " .. q .. " to replace it, or a different title"
		)
	end
	local written = {
		id = target,
		title = title,
		description = description,
		type = kind,
		tags = clean_tags,
		created = existing and existing.created or now,
		updated = now,
		valid_from = existing and existing.valid_from or now,
		source = draft.source or note.source("agent"),
		links = clean_links,
		body = body,
	}
	written.source.files = written.source.files or {}
	local action, ok, err
	if draft.supersedes ~= nil then
		ok, err = self.notes:supersede(draft.supersedes, note.copy(written), now)
		action = "superseded"
	elseif existing then
		ok, err = self.notes:update(note.copy(written))
		action = "updated"
	else
		ok, err = self.notes:create(note.copy(written))
		action = "created"
	end
	if not ok then
		return failed(err)
	end
	local stored = self.notes:get(target)
	self.index:upsert(target, bm25.index_text(stored))
	if draft.supersedes ~= nil then
		local old = self.notes:get(draft.supersedes)
		if old then
			self.index:upsert(old.id, bm25.index_text(old))
		end
	end
	local nearest = {}
	for _, hit in ipairs(recall.recall(self.notes, self.index, stored.title .. " " .. stored.description, M.NEAREST + 1)) do
		if hit.id ~= target and #nearest < M.NEAREST then
			table.insert(nearest, hit)
		end
	end
	return {
		id = target,
		action = action,
		superseded = draft.supersedes,
		redacted = redacted,
		nearest = nearest,
	}
end

-- Links `from` to `to`, typed. The target may be a note not written
-- yet; for `about`, it is a path.
function Memory:link(from, to, kind, why, now)
	local n = self.notes:get(from)
	if n == nil then
		return refused("no note is called " .. store.quoted(from))
	end
	if why ~= nil then
		local refusal = safety.refusal(why)
		if refusal then
			return refused("not stored: " .. refusal)
		end
		why = (safety.redact(why))
	end
	if kind ~= "about" and not note.is_id(to) then
		return refused(store.quoted(to) .. " is not a note id")
	end
	n = note.copy(n)
	local kept = {}
	for _, link in ipairs(n.links) do
		if link.to ~= to then
			table.insert(kept, link)
		end
	end
	table.insert(kept, { to = to, type = kind, why = why })
	n.links = kept
	n.updated = now
	local ok, err = self.notes:update(n)
	if not ok then
		return failed(err)
	end
	return true
end

-- Up to `budget` hits for `query`.
function Memory:search(query, budget)
	return recall.recall(self.notes, self.index, query, budget)
end

-- A note, with the links into it: `{ note, backlinks }`, or nil.
function Memory:read(id)
	local n = self.notes:get(id)
	if n == nil then
		return nil
	end
	return { note = n, backlinks = self.notes:backlinks(id) }
end

-- Marks the notes about any of `paths`, last written at or before
-- `written_by`, as possibly stale.
function Memory:mark_stale(paths, why, written_by, now)
	local marked, err = self.notes:mark_stale(paths, why, written_by, now)
	if marked == nil then
		return nil, err.message
	end
	-- What a mark changed is searched as written now.
	for _, id in ipairs(marked) do
		self.index:upsert(id, bm25.index_text(self.notes:get(id)))
	end
	return marked
end

-- The folders `uses.files` names the scopes by.
M.REPOSITORY = "repository/" .. M.FOLDER
M.DATA = "data/" .. M.FOLDER

-- The folders a run's scopes are in, where the host has them: the
-- repository's own, and the user's, under tau's data folder.
function M.folders(ctx)
	local roots = ctx.files.roots()
	return { repository = roots[M.REPOSITORY], user = roots[M.DATA] }
end

-- The run's scopes, opened: `{ repository, user }`.
function M.scopes(ctx)
	local folders = M.folders(ctx)
	local scopes = {}
	if folders.repository then
		scopes.repository = M.open(ctx.files, folders.repository)
	end
	if folders.user then
		scopes.user = M.open(ctx.files, folders.user)
	end
	return scopes
end

function M.write(scopes, name, draft, now)
	local scope = scopes[name]
	if scope == nil then
		return nil, "there is no " .. name .. " scope", "refused"
	end
	return scope:write(draft, now)
end

-- Up to `limit` hits across both scopes: the repository's first, then
-- the user's, each list best first, user ids prefixed.
function M.search(scopes, query, limit)
	local hits = {}
	if scopes.repository then
		for _, hit in ipairs(scopes.repository:search(query, limit)) do
			table.insert(hits, hit)
		end
	end
	if scopes.user then
		for _, hit in ipairs(scopes.user:search(query, limit)) do
			hit.id = M.USER .. hit.id
			if hit.via then
				hit.via.from = M.USER .. hit.via.from
			end
			table.insert(hits, hit)
		end
	end
	while #hits > limit do
		table.remove(hits)
	end
	return hits
end

return M
