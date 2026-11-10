-- Search and recall: BM25 against its formula written out plainly, and
-- recall's budget, link hops and ranking as laws over random notes.
local tau = require("tau")
local t = tau.test
local g = t.gen
local bm25 = require("bm25")
local note = require("note")
local recall = require("recall")
local store = require("store")
local text = require("text")
local drawn = require("drawn")

local VOCAB = { "retry", "after", "header", "jitter", "cache", "lane", "delta", "fork" }

local function words_text(draw)
	return table.concat(draw(g.lists(g.sampled_from(VOCAB), { max = 12 })), " ")
end

-- BM25 the plain way, in doubles: every term of the query, every
-- document.
local function reference(docs, query)
	local tokens, n, total = {}, 0, 0
	for id, s in pairs(docs) do
		tokens[id] = text.words(s)
		n += 1
		total += #tokens[id]
	end
	local avg = total / n
	local terms, seen = {}, {}
	for _, word in ipairs(text.words(query)) do
		if not seen[word] then
			seen[word] = true
			table.insert(terms, word)
		end
	end
	local scores = {}
	for id, doc in pairs(tokens) do
		local score = 0
		for _, term in ipairs(terms) do
			local tf = 0
			for _, w in ipairs(doc) do
				if w == term then
					tf += 1
				end
			end
			if tf > 0 then
				local df = 0
				for _, other in pairs(tokens) do
					if table.find(other, term) then
						df += 1
					end
				end
				local idf = math.log((n - df + 0.5) / (df + 0.5) + 1)
				local norm = 1.2 * (1 - 0.75 + 0.75 * #doc / math.max(avg, 1))
				score += idf * tf * (1.2 + 1) / (tf + norm)
			end
		end
		if score > 0 then
			table.insert(scores, { id = id, score = score })
		end
	end
	bm25.rank(scores)
	return scores
end

t.property("bm25 scores as its formula", function(draw)
	local index = bm25.new()
	local docs = {}
	-- Documents written, some rewritten and some removed, as notes are.
	for _ = 1, draw(g.integers({ min = 0, max = 12 })) do
		local id = draw(g.sampled_from({ "a", "b", "c", "d", "e", "f" }))
		if draw(g.booleans()) or docs[id] == nil then
			local s = words_text(draw)
			index:upsert(id, s)
			docs[id] = s
		else
			index:remove(id)
			docs[id] = nil
		end
	end
	local query = words_text(draw)
	local got = index:search(query, 100)
	local want = next(docs) and reference(docs, query) or {}
	t.equal(#got, #want, "as many hits")
	for i, hit in ipairs(got) do
		local close = math.abs(hit.score - want[i].score) <= 1e-4 * math.max(want[i].score, 1)
		t.equal(close, true, hit.id .. ": " .. hit.score .. " vs " .. want[i].id .. ": " .. want[i].score)
	end
	-- The same order, where scores differ by more than rounding.
	local at = {}
	for i, hit in ipairs(got) do
		at[hit.id] = i
	end
	for i = 1, #want - 1 do
		if want[i].score - want[i + 1].score > 1e-3 then
			t.equal(at[want[i].id] < at[want[i + 1].id], true, "order")
		end
	end
end, { cases = 200 })

-- Notes from the pool, with bodies drawn from a small vocabulary, in a
-- store and an index.
local function setup(draw)
	local notes = store.open(drawn.files(), "/n")
	local index = bm25.new()
	-- Sometimes no note links anywhere, so seeds are left to fill the
	-- budget.
	local unlinked = draw(g.booleans({ p = 0.2 }))
	for _ = 1, draw(g.integers({ min = 1, max = 8 })) do
		local n = draw(drawn.pool_note(g))
		n.body = n.body .. "\n" .. words_text(draw)
		if unlinked then
			n.links = {}
			n.body = words_text(draw)
		end
		if draw(g.booleans()) then
			n.valid_to = 5
		end
		if notes:create(note.copy(n)) then
			index:upsert(n.id, bm25.index_text(n))
		end
	end
	return notes, index
end

-- The seeds recall starts from: what the index answers for `limit`,
-- notes that are gone left out, superseded ones weighed down, best
-- first and ties by id.
local function weighted(notes, index, query, limit)
	local seeds = {}
	for _, hit in ipairs(index:search(query, limit)) do
		local n = notes:get(hit.id)
		if n then
			local score = hit.score
			if note.is_superseded(n) then
				score = bm25.f32(score * recall.SUPERSEDED_WEIGHT)
			end
			table.insert(seeds, { id = hit.id, score = score })
		end
	end
	bm25.rank(seeds)
	local ids = {}
	for _, seed in ipairs(seeds) do
		table.insert(ids, seed.id)
	end
	return ids
end

-- Recall keeps its budget and lists each note once; seeds come in their
-- weighted order, and each linked note after the seed it hangs from;
-- and the budget fills when there is enough to fill it with.
t.property("recall keeps its budget and follows links", function(draw)
	local notes, index = setup(draw)
	local query = words_text(draw)
	local budget = draw(g.one_of({ g.integers({ min = 0, max = 3 }), g.integers({ min = 0, max = 6 }) }))
	local hits = recall.recall(notes, index, query, budget)
	t.equal(#hits <= budget, true, "within the budget")
	local ids, seen = {}, {}
	for _, hit in ipairs(hits) do
		t.equal(seen[hit.id], nil, "each note once")
		seen[hit.id] = true
		table.insert(ids, hit.id)
	end
	-- Recall asks the index for three times its budget.
	local seeds = weighted(notes, index, query, budget * 3)
	local last_seed = 0
	for at, hit in ipairs(hits) do
		local n = notes:get(hit.id)
		t.equal(hit.superseded, note.is_superseded(n))
		if hit.via == nil then
			-- A seed, after the seeds that outrank it.
			local rank = table.find(seeds, hit.id)
			t.equal(rank ~= nil, true, "search found it")
			t.equal(rank > last_seed, true, "in the seeds' order")
			last_seed = rank
		else
			-- Reached by a link from a seed listed before it.
			local seed = table.find(ids, hit.via.from)
			t.equal(seed ~= nil and seed < at and hits[seed].via == nil, true, "its seed is listed before it")
			local found = false
			for _, other in ipairs(recall.linked(notes, hit.via.from)) do
				found = found or (other.id == hit.id and other.type == hit.via.type)
			end
			t.equal(found, true, "linked so")
		end
	end
	-- A third of the budget is kept for linked notes, from the seeds that
	-- come first; whatever they leave, later seeds fill.
	local first = math.min(#seeds, budget - budget // 3)
	local candidates, count = {}, 0
	local function add(id)
		if not candidates[id] then
			candidates[id] = true
			count += 1
		end
	end
	for _, id in ipairs(seeds) do
		add(id)
	end
	for i = 1, first do
		for _, other in ipairs(recall.linked(notes, seeds[i])) do
			add(other.id)
		end
	end
	t.equal(#hits, math.min(budget, count), "the budget fills")
end, { cases = 200 })

local function seeds_of(notes, index, query)
	local out = {}
	for _, hit in ipairs(recall.recall(notes, index, query, 12)) do
		if hit.via == nil then
			table.insert(out, hit.id)
		end
	end
	return out
end

-- Superseding a note never raises it, and leaves the others in their
-- order.
t.property("superseding a note never raises it", function(draw)
	local notes, index = setup(draw)
	local query = words_text(draw)
	local current = {}
	for _, id in ipairs(seeds_of(notes, index, query)) do
		if not note.is_superseded(notes:get(id)) then
			table.insert(current, id)
		end
	end
	t.assume(#current > 0)
	local id = draw(g.sampled_from(current))
	local before = seeds_of(notes, index, query)
	local n = note.copy(notes:get(id))
	n.valid_to = 9
	notes:update(n)
	local after = seeds_of(notes, index, query)
	t.equal(table.find(after, id) >= table.find(before, id), true, "it did not rise")
	local function others(list)
		local out = {}
		for _, other in ipairs(list) do
			if other ~= id then
				table.insert(out, other)
			end
		end
		return out
	end
	t.equal(others(after), others(before), "the others kept their order")
end, { cases = 200 })

-- A note with nothing but an id and a description.
local function plain(id, description)
	return {
		id = id,
		title = "about " .. id,
		description = description,
		type = "fact",
		tags = {},
		created = 1,
		updated = 1,
		valid_from = 1,
		source = note.source("agent"),
		links = {},
		body = "",
	}
end

local function only_one_holds(drawn_notes, pick)
	local notes = store.open(drawn.files(), "/n")
	local index = bm25.new()
	for _, n in ipairs(drawn_notes) do
		if notes:create(note.copy(n)) then
			index:upsert(n.id, bm25.index_text(n))
		end
	end
	local ids = notes:ids()
	local id = ids[pick(#ids)]
	local n = note.copy(notes:get(id))
	n.body ..= "\nzanzibar_only_here"
	n.valid_to = nil
	notes:update(n)
	index:upsert(n.id, bm25.index_text(n))
	-- The word alone: any other word could be one a drawn note holds.
	local hits = recall.recall(notes, index, "zanzibar_only_here", 3)
	t.equal(hits[1].id, n.id)
	t.equal(hits[1].snippet, "zanzibar_only_here")
end

-- A word only one note holds finds that note first, with the line that
-- holds it, and nothing else, as the snippet.
t.property("a word only one note holds finds it first", function(draw)
	local list = draw(g.lists(drawn.pool_note(g), { min = 1, max = 8 }))
	only_one_holds(list, function(n)
		return draw(g.integers({ min = 1, max = n }))
	end)
end)

t.case("a word only one note holds finds it first: two plain notes", function()
	only_one_holds({ plain("a", "retry the header"), plain("b", "a cache") }, function()
		return 1
	end)
end)

-- Words of the snippet property: a few the query may hold, some it never
-- does, and characters of several bytes.
local SNIPPET_WORDS = { "retry", "header", "cache", "lane", "the", "of", "é", "日本" }

-- A hit's snippet is the body line holding the most of the query's
-- words, trimmed, and cut to 200 characters (not bytes) and an ellipsis
-- when longer; with no line holding any, it is the note's description.
-- Lines tie on purpose: any of the tied lines may be the snippet. Line
-- lengths gather around the cut.
t.property("a snippet is the line holding most of the query", function(draw)
	local lines = draw(g.lists(
		g.one_of({
			g.lists(g.sampled_from(SNIPPET_WORDS), { max = 6 }):map(function(words)
				return table.concat(words, " ")
			end),
			-- A long line: some words, then `é` to a length about the cut.
			g.composite(function(d)
				local line = table.concat(d(g.lists(g.sampled_from(SNIPPET_WORDS), { max = 3 })), " ") .. " "
				local length = d(g.sampled_from({ 199, 200, 201, 260 }))
				while text.length(line) < length do
					line ..= "é"
				end
				return line
			end),
		}),
		{ max = 5 }
	))
	local pad = draw(g.sampled_from({ "", "  " }))
	local padded = {}
	for _, line in ipairs(lines) do
		table.insert(padded, pad .. line .. pad)
	end
	local asked = draw(g.lists(g.sampled_from({ "retry", "header", "cache", "lane" }), { min = 1, max = 3 }))
	-- `anchor` is in the description alone, so the note is always found.
	local query = "anchor " .. table.concat(asked, " ")

	local notes = store.open(drawn.files(), "/n")
	local n = plain("only", "the anchor note")
	n.body = table.concat(padded, "\n")
	notes:create(note.copy(n))
	local index = bm25.new()
	index:upsert(n.id, bm25.index_text(n))
	local hits = recall.recall(notes, index, query, 3)
	t.equal(#hits, 1)

	local terms = text.words(query)
	local function held(line)
		local c = 0
		for _, word in ipairs(text.words(line)) do
			if table.find(terms, word) then
				c += 1
			end
		end
		return c
	end
	local most = 0
	for _, line in ipairs(lines) do
		most = math.max(most, held(line))
	end
	local function shown(line)
		line = text.trim(line)
		local cut, more = text.prefix(line, 200)
		return more and (cut .. "…") or line
	end
	local snippet = hits[1].snippet
	if most == 0 then
		t.equal(snippet, n.description)
	else
		local tied = {}
		for _, line in ipairs(lines) do
			if held(line) == most then
				table.insert(tied, shown(line))
			end
		end
		t.equal(table.find(tied, snippet) ~= nil, true, snippet)
	end
end, { cases = 200 })
