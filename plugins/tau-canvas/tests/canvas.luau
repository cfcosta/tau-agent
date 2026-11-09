-- The canvas itself: edits refuse what is missing or ambiguous and
-- replace only their own text, in order; every revision is kept in order,
-- each item numbered apart; and the pane shows what was picked until
-- something newer comes.
local tau = require("tau")
local t = tau.test
local g = t.gen
local canvas = require("canvas")

t.case("edits refuse what is missing or ambiguous", function()
	local text = "one two two three"
	local function why(edits)
		local done, refused = canvas.apply_edits(text, edits)
		t.equal(done, nil)
		return refused
	end
	t.equal(why({}), "No edits were given.")
	t.equal(why({ { old = "", new = "x" } }), "Edit 1: `old` is empty.")
	t.equal(why({ { old = "four", new = "x" } }), "Edit 1: `old` does not occur in the document.")
	t.equal(
		why({ { old = "one", new = "1" }, { old = "two", new = "2" } }),
		"Edit 2: `old` occurs more than once in the document; include more of the text around it."
	)
	-- An edit sees the text the edits before it left.
	t.equal(
		canvas.apply_edits(text, { { old = "two two", new = "2" }, { old = "2 three", new = "end" } }),
		"one end"
	)
	-- Matches do not overlap: "aa" is in "aaa" once.
	t.equal(canvas.apply_edits("aaa", { { old = "aa", new = "b" } }), "ba")
end)

-- A document of distinct words, and an edit of one of them.
t.property("an edit replaces only its own text, in order", function(draw)
	local words = draw(g.lists(g.from_regex("[a-z]{1,6}"), { min = 1, max = 8, unique = true }))
	local text = table.concat(words, " ")
	local at = draw(g.integers({ min = 1, max = #words }))
	local new = draw(g.from_regex("[A-Z]{0,4}"))
	t.assume(new ~= words[at])
	-- Each word is whole in the text once: give it its spaces.
	local padded = " " .. text .. " "
	local old = " " .. words[at] .. " "
	t.assume(select(2, string.gsub(padded, old, "")) == 1)
	local done = canvas.apply_edits(padded, { { old = old, new = " " .. new .. " " } })
	local want = {}
	for n, word in ipairs(words) do
		table.insert(want, n == at and new or word)
	end
	t.equal(done, " " .. table.concat(want, " ") .. " ")
end)

t.property("the canvas keeps every revision in order, each item apart", function(draw)
	local of = canvas.of({})
	local made = {}
	local steps = draw(g.integers({ min = 1, max = 12 }))
	for n = 1, steps do
		local again = #of.items > 0 and draw(g.booleans())
		local id = again and of.items[draw(g.integers({ min = 1, max = #of.items }))].id or canvas.next_id(of)
		local text = "revision " .. n
		canvas.revise(of, "call_" .. n, id, "title " .. n, { type = "document", text = text })
		made[id] = made[id] or {}
		table.insert(made[id], text)
		t.equal(of.fresh, n)
		t.equal(of.last, id)
		local item, at = canvas.by_call(of, "call_" .. n)
		t.equal(item.id, id)
		t.equal(at, #made[id])
	end
	for _, item in ipairs(of.items) do
		local texts = {}
		for _, revision in ipairs(item.revisions) do
			table.insert(texts, revision.content.text)
		end
		t.equal(texts, made[item.id])
	end
	local ids = {}
	for _, item in ipairs(of.items) do
		table.insert(ids, item.id)
	end
	for n, id in ipairs(ids) do
		t.equal(id, n)
	end
end)

t.case("the pane shows what was picked until something newer comes", function()
	local of = canvas.of({})
	canvas.revise(of, "c1", 1, "Plan", { type = "document", text = "a" })
	canvas.revise(of, "c2", 1, "Plan", { type = "document", text = "b" })
	canvas.revise(of, "c3", 2, "Chart", { type = "document", text = "c" })
	local item, at = canvas.shown(of, nil)
	t.equal({ item.id, at }, { 2, 1 })
	item, at = canvas.shown(of, { item = 1, revision = 1, fresh = of.fresh })
	t.equal({ item.id, at }, { 1, 1 })
	-- Past the last revision is the last.
	item, at = canvas.shown(of, { item = 1, revision = 9, fresh = of.fresh })
	t.equal({ item.id, at }, { 1, 2 })
	canvas.revise(of, "c4", 2, "Chart", { type = "document", text = "d" })
	item, at = canvas.shown(of, { item = 1, revision = 1, fresh = of.fresh - 1 })
	t.equal({ item.id, at }, { 2, 2 })
end)

t.case("words are counted and grouped by thousands", function()
	t.equal(canvas.words("  one two\nthree  "), 3)
	t.equal(canvas.grouped(7), "7")
	t.equal(canvas.grouped(1234), "1,234")
	t.equal(canvas.grouped(1234567), "1,234,567")
	t.equal(canvas.grouped(100000), "100,000")
end)

t.case("a card's excerpt is a document's first lines, plainly", function()
	t.equal(
		canvas.excerpt("# Title\n\n`code` here\n  ## Sub  \none\ntwo\nthree\nfour\nfive", 6),
		{ "Title", "code here", "## Sub", "one", "two", "three" }
	)
end)
