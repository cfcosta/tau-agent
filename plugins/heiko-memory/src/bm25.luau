-- What finds notes by their words: Okapi BM25 over whole notes (ADR
-- 0052), with the usual `k1 = 1.2`, `b = 0.75`. Scores are worked out
-- in single precision, as the Rust index did, each step rounded, so the
-- same notes rank the same. The index is derived: rebuilt from the
-- notes at any time.
local text = require("text")

local M = {}

-- `x` rounded to single precision.
local function f32(x)
	return (string.unpack("f", string.pack("f", x)))
end
M.f32 = f32

M.K1 = f32(1.2)
M.B = 0.75

-- What an index holds for a note: its title, description, tags and body.
function M.index_text(note)
	return note.title .. "\n" .. note.description .. "\n" .. table.concat(note.tags or {}, " ") .. "\n" .. note.body
end

local Index = {}
Index.__index = Index

function M.new()
	return setmetatable({ docs = {}, df = {}, total = 0, count = 0 }, Index)
end

function Index:remove(id)
	local doc = self.docs[id]
	if doc == nil then
		return
	end
	self.docs[id] = nil
	self.count -= 1
	self.total -= doc.len
	for word in pairs(doc.counts) do
		local n = self.df[word] - 1
		self.df[word] = n > 0 and n or nil
	end
end

-- Adds a note's text, or replaces it.
function Index:upsert(id, s)
	self:remove(id)
	local counts, len = {}, 0
	for _, word in ipairs(text.words(s)) do
		counts[word] = (counts[word] or 0) + 1
		len += 1
	end
	for word in pairs(counts) do
		self.df[word] = (self.df[word] or 0) + 1
	end
	self.total += len
	self.count += 1
	self.docs[id] = { counts = counts, len = len }
end

-- Sorts `hits`, `{ id, score }` each, best first, ties by id.
function M.rank(hits)
	table.sort(hits, function(a, b)
		if a.score ~= b.score then
			return a.score > b.score
		end
		return a.id < b.id
	end)
end

-- Up to `limit` hits for `query`, `{ id, score }` each, best first.
function Index:search(query, limit)
	if self.count == 0 then
		return {}
	end
	local n = f32(self.count)
	local avg = f32(f32(self.total) / n)
	local norm_avg = math.max(avg, 1)
	local terms, seen = {}, {}
	for _, word in ipairs(text.words(query)) do
		if not seen[word] then
			seen[word] = true
			table.insert(terms, word)
		end
	end
	table.sort(terms)
	local K1, B = M.K1, M.B
	local K1_1 = f32(K1 + 1)
	local hits = {}
	for id, doc in pairs(self.docs) do
		local score = 0
		for _, term in ipairs(terms) do
			local tf = doc.counts[term]
			if tf then
				local df = self.df[term]
				local idf = f32(math.log(f32(f32(f32(f32(n - df) + 0.5) / f32(df + 0.5)) + 1)))
				local norm = f32(K1 * f32(f32(1 - B) + f32(f32(B * doc.len) / norm_avg)))
				score = f32(score + f32(f32(f32(idf * tf) * K1_1) / f32(tf + norm)))
			end
		end
		if score > 0 then
			table.insert(hits, { id = id, score = score })
		end
	end
	M.rank(hits)
	local out = {}
	for i = 1, math.min(limit, #hits) do
		out[i] = hits[i]
	end
	return out
end

return M
