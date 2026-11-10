-- Finding notes for a query: the index for seed notes, then one hop
-- along links, inside one budget (`docs/research/memory.md`,
-- "Reading").
--
-- Search finds entry points; links carry what search misses, such as a
-- decision that names neither of a query's words. Superseded notes rank
-- lower and are labelled, never hidden: history is often the answer.
local bm25 = require("bm25")
local note = require("note")
local text = require("text")

local M = {}

-- How much a superseded note's score counts against a current one's.
M.SUPERSEDED_WEIGHT = 0.5

local SNIPPET = 200

-- The notes linked with `id` either way, each once with the first link
-- found: its own links first, then links into it, as `{ id, type }`.
-- Paths (`about`) and notes not written yet are skipped.
function M.linked(notes, id)
	local found = {}
	local function add(other, kind)
		if other == id or kind == "about" or notes:get(other) == nil then
			return
		end
		for _, seen in ipairs(found) do
			if seen.id == other then
				return
			end
		end
		table.insert(found, { id = other, type = kind })
	end
	local n = notes:get(id)
	if n then
		for _, link in ipairs(note.all_links(n)) do
			add(link.to, link.type)
		end
	end
	for _, back in ipairs(notes:backlinks(id)) do
		add(back.from, back.link.type)
	end
	return found
end

-- The body line holding most of the query's words, trimmed and cut to
-- 200 characters; nil when no line holds any. Of lines that tie, the
-- last.
function M.snippet(body, query)
	local terms = {}
	for _, word in ipairs(text.words(query)) do
		terms[word] = true
	end
	local best, most = nil, -1
	for _, line in ipairs(text.lines(body)) do
		local held = 0
		for _, word in ipairs(text.words(line)) do
			if terms[word] then
				held += 1
			end
		end
		if held >= most then
			best, most = line, held
		end
	end
	if best == nil or most == 0 then
		return nil
	end
	local line = text.trim(best)
	local cut, more = text.prefix(line, SNIPPET)
	if more then
		return cut .. "…"
	end
	return line
end

-- Up to `budget` notes for `query`: the best search hits (superseded ones
-- weighed down), then the notes linked with them, one hop, in the seeds'
-- order. A third of the budget is kept for linked notes; seeds fill
-- whatever links leave free. Each hit is `{ id, title, type,
-- description, snippet, superseded, stale, via }`, `via` `{ from, type }`
-- for a note reached by a link.
function M.recall(notes, index, query, budget)
	if budget == 0 then
		return {}
	end
	local seeds = {}
	for _, hit in ipairs(index:search(query, budget * 3)) do
		-- The index can lag the files; a note that is gone is skipped.
		local n = notes:get(hit.id)
		if n then
			local score = hit.score
			if note.is_superseded(n) then
				score = bm25.f32(score * M.SUPERSEDED_WEIGHT)
			end
			table.insert(seeds, { id = hit.id, score = score })
		end
	end
	bm25.rank(seeds)

	local reserve = budget // 3
	local first = math.min(#seeds, budget - reserve)
	local picked = {}
	local taken = {}
	local function pick(id, via)
		table.insert(picked, { id = id, via = via })
		taken[id] = true
	end
	for i = 1, first do
		pick(seeds[i].id, nil)
	end
	for i = 1, first do
		local seed = seeds[i].id
		local full = false
		for _, other in ipairs(M.linked(notes, seed)) do
			if #picked >= budget then
				full = true
				break
			end
			if not taken[other.id] then
				pick(other.id, { from = seed, type = other.type })
			end
		end
		if full then
			break
		end
	end
	for i = first + 1, #seeds do
		if #picked >= budget then
			break
		end
		if not taken[seeds[i].id] then
			pick(seeds[i].id, nil)
		end
	end

	local hits = {}
	for _, p in ipairs(picked) do
		local n = notes:get(p.id)
		if n then
			table.insert(hits, {
				id = p.id,
				title = n.title,
				type = n.type,
				description = n.description,
				snippet = M.snippet(n.body, query) or n.description,
				superseded = note.is_superseded(n),
				stale = n.stale,
				via = p.via,
			})
		end
	end
	return hits
end

return M
