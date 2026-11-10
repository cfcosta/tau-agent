-- A note and its file: round trips, links in the body, ids from titles,
-- and what `validate` refuses.
local tau = require("tau")
local t = tau.test
local g = t.gen
local note = require("note")
local drawn = require("drawn")

local ASCII_WORDS = "[a-zA-Z0-9]"

t.property("a note reads back as written", function(draw)
	local n = draw(drawn.note(g))
	t.equal(note.validate(n), nil, "valid")
	local file = note.render(n)
	local back, why = note.parse(file)
	t.equal(why, nil, "it reads")
	t.equal(back, n, "the note")
	-- And its file does not change on a second trip.
	t.equal(note.render(back), file, "the file")
end, { cases = 300 })

-- A bare `[[id]]` in the body relates the note to `id`, once however
-- often it appears, unless the front matter links there already: then
-- the links there are the listed ones, as listed, a repeat included.
t.property("a bare link in the body relates", function(draw)
	local n = draw(drawn.note(g))
	local target = draw(drawn.id(g))
	n.body = "See [[" .. target .. "]] and [[" .. target .. "]], not [[Not An Id]]."
	local bare, listed = {}, {}
	for _, link in ipairs(note.all_links(n)) do
		if link.to == target then
			table.insert(bare, link)
		end
		t.equal(link.to ~= "Not An Id", true, "no link to what is not an id")
	end
	for _, link in ipairs(n.links) do
		if link.to == target then
			table.insert(listed, link)
		end
	end
	if #listed == 0 then
		t.equal(bare, { { to = target, type = "relates" } })
	else
		t.equal(bare, listed)
	end
end)

-- The slug the plain way: lowercase words joined by `-`, whole words
-- only, while they fit in 64 bytes.
local function expected_slug(words)
	local out = ""
	for _, word in ipairs(words) do
		local sep = out == "" and 0 or 1
		if #out + sep + #word > 64 then
			break
		end
		if sep == 1 then
			out ..= "-"
		end
		out ..= string.lower(word)
	end
	return out == "" and "note" or out
end

t.property("slug matches the independent whole-word prefix", function(draw)
	local first = "x" .. draw(g.from_regex("^" .. ASCII_WORDS .. "{0,63}$"))
	local words = { first }
	for _, word in ipairs(draw(g.lists(g.from_regex("^" .. ASCII_WORDS .. "{1,70}$"), { max = 5 }))) do
		table.insert(words, word)
	end
	local separator = draw(g.sampled_from({ " ", "-", "_", ".", "/", ":", "é", "雪", "\t" }))
	local title = table.concat(words, separator)
	local want = expected_slug(words)
	t.equal(want ~= "note", true, "the first word does not fall back")
	local got = note.slug(title)
	t.equal(got, want, title)
	t.equal(note.is_id(got), true, got)
	t.equal(note.slug(got), got, "idempotent")
end)

t.case("slug obeys 64-byte whole-word boundaries and its fallback", function()
	local cases = {
		{ string.rep("A", 63), string.rep("a", 63) },
		{ string.rep("B", 64), string.rep("b", 64) },
		{ string.rep("C", 65), "note" },
		{ string.rep("D", 65) .. " short", "note" },
		{ string.rep("A", 30) .. " " .. string.rep("B", 33), string.rep("a", 30) .. "-" .. string.rep("b", 33) },
		{
			string.rep("A", 20) .. " " .. string.rep("B", 20) .. " " .. string.rep("C", 25),
			string.rep("a", 20) .. "-" .. string.rep("b", 20),
		},
		{ "é 雪", "note" },
	}
	for _, case in ipairs(cases) do
		local got = note.slug(case[1])
		t.equal(got, case[2], case[1])
		t.equal(note.is_id(got), true, got)
		t.equal(note.slug(got), got, "idempotent")
	end
end)

-- `[[id]]` links come back in order, each once; brackets around what
-- is not an id, and an unclosed pair, give nothing.
t.property("wiki links are the ids in brackets", function(draw)
	local parts = draw(g.lists(
		g.one_of({
			drawn.id(g):map(function(id)
				return { text = "[[" .. id .. "]]", id = id }
			end),
			g.from_regex("^[a-z .]{0,8}$"):map(function(s)
				return { text = s }
			end),
			g.sampled_from({ "[[Not An Id]]", "[[]]", "[[a b]]" }):map(function(s)
				return { text = s }
			end),
		}),
		{ max = 8 }
	))
	local tail = draw(g.sampled_from({ "", "[[x", "[[y]" }))
	local texts, want = {}, {}
	for _, part in ipairs(parts) do
		table.insert(texts, part.text)
		if part.id and not table.find(want, part.id) then
			table.insert(want, part.id)
		end
	end
	local s = table.concat(texts) .. tail
	t.equal(note.wiki_links(s), want, s)
end)

-- A line break in any front-matter field would let a value close the
-- front matter or start another key, so `validate` refuses it in every
-- field, whichever break it is and wherever in the value it falls.
t.property("a line break in any front-matter field is refused", function(draw)
	local n = draw(drawn.note(g))
	t.equal(note.validate(n), nil, "valid to begin with")
	local broken = draw(drawn.line(g)) .. draw(g.sampled_from({ "\n", "\r", "\r\n", "\n+++\n" })) .. draw(g.text({ max = 10 }))
	local link = { to = draw(drawn.id(g)), type = "relates" }
	local field = draw(g.integers({ min = 0, max = 8 }))
	if field == 0 then
		n.title = broken
	elseif field == 1 then
		n.description = broken
	elseif field == 2 then
		table.insert(n.tags, broken)
	elseif field == 3 then
		n.stale = broken
	elseif field == 4 then
		n.source.run = broken
	elseif field == 5 then
		n.source.commit = broken
	elseif field == 6 then
		table.insert(n.source.files, broken)
	elseif field == 7 then
		link.to = broken
		table.insert(n.links, link)
	else
		link.why = broken
		table.insert(n.links, link)
	end
	t.equal(note.validate(n) ~= nil, true, "refused")
end, { cases = 300 })

-- The fields that name the note cannot be blank either.
t.property("a blank title or description is refused", function(draw)
	local n = draw(drawn.note(g))
	local blank = draw(g.from_regex("^[ \t]{0,5}$"))
	if draw(g.booleans()) then
		n.title = blank
	else
		n.description = blank
	end
	t.equal(note.validate(n) ~= nil, true, "refused")
end)

-- A note's file can be edited by hand: comments, other quoting, inline
-- tables, dotted keys and keys tau does not know all read.
t.case("a note edited by hand still reads", function()
	local file = table.concat({
		"+++",
		"# a comment",
		"id = 'retry-after'",
		'title = """retry-after can be an "HTTP date\\""""',
		"description = \"Parse both forms\" # trailing",
		"type = \"gotcha\"",
		"tags = [",
		"  'http', # one",
		"  \"retry\",",
		"]",
		"created = 1_790_000_000_000",
		"updated = 1790000000000",
		"valid_from = 0x10",
		"reviewed = 2026-11-10T10:00:00Z",
		"source = { by = \"user\", files = [\"src/retry.rs\"] }",
		"links = [{ to = \"retry-policy\", type = \"refines\", why = 'adds detail' }]",
		"+++",
		"Body.",
	}, "\n")
	local n, why = note.parse(file)
	t.equal(why, nil)
	t.equal(n.title, 'retry-after can be an "HTTP date"')
	t.equal(n.tags, { "http", "retry" })
	t.equal(n.created, 1790000000000)
	t.equal(n.valid_from, 16)
	t.equal(n.source, { by = "user", files = { "src/retry.rs" } })
	t.equal(n.links, { { to = "retry-policy", type = "refines", why = "adds detail" } })
	t.equal(n.body, "Body.")
end)

-- What does not read as a note says why, and reads as nothing.
t.case("a file that is no note says why", function()
	local function why(file)
		local n, reason = note.parse(file)
		t.equal(n, nil, file)
		return reason
	end
	t.equal(why("no front matter"), "the file does not start with +++ front matter")
	t.equal(why("+++\nid = 'x'\n"), "the file does not start with +++ front matter")
	local base = "id = 'x'\ntitle = 't'\ndescription = 'd'\ncreated = 1\nupdated = 1\nvalid_from = 1\n[source]\nby = 'agent'\n"
	t.equal(note.parse("+++\n" .. base .. "type = 'todo'\n+++\n") == nil, true, "an unknown type")
	t.equal(note.parse("+++\ntype = 'fact'\n" .. base .. "+++\n") ~= nil, true, "the base reads")
	t.equal(note.parse("+++\ntype = 'fact'\n" .. string.gsub(base, "created = 1", "created = -1") .. "+++\n") == nil, true, "a negative time")
	t.equal(note.parse("+++\ntype = 'fact'\ntype = 'case'\n" .. base .. "+++\n") == nil, true, "a key twice")
	t.equal(string.find(why("+++\ntitle = 'unclosed\n+++\n"), "bad front matter", 1, true) ~= nil, true)
end)
