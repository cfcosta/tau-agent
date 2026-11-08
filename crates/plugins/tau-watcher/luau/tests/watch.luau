-- The watcher's own rules: the reply it reads, the cadence, what the
-- chat is given, and the notes it keeps.
local tau = require("tau")
local t = tau.test
local g = t.gen
local watch = require("watch")

local NOTE = table.concat({
	"learn: `cargo test` rebuilds in debug, so each run takes minutes.",
	"tag: Heads up",
	"explain:",
	"**Debug tests rebuild everything**",
	"- `cargo test` builds in its own profile.",
	"- Nothing from `--release` is reused.",
	"- Each run recompiles every crate.",
}, "\n")

t.case("none is nothing however it is spelled", function()
	for _, text in ipairs({ "learn: none", "  learn: none\n", "Learn: None.", "learn:none\n\n" }) do
		t.equal(watch.parse(text), { kind = "nothing" }, text)
	end
end)

t.case("a note with an explanation reads", function()
	t.equal(watch.parse(NOTE), {
		kind = "note",
		tag = "Heads up",
		line = "`cargo test` rebuilds in debug, so each run takes minutes.",
		explain = {
			title = "Debug tests rebuild everything",
			bullets = {
				"`cargo test` builds in its own profile.",
				"Nothing from `--release` is reused.",
				"Each run recompiles every crate.",
			},
		},
	})
end)

t.case("a quoted or fenced reply is not read", function()
	for _, text in ipairs({ "```\nlearn: none\n```", "> learn: none", '"learn: none"', "`learn: none`" }) do
		local reply, why = watch.parse(text)
		t.equal(reply, nil, text)
		t.equal(why, watch.UNREADABLE.quoted, text)
	end
end)

t.case("prose and blanks are not read", function()
	local function why(text)
		local _, because = watch.parse(text)
		return because
	end
	t.equal(why(""), watch.UNREADABLE.empty)
	t.equal(why("  \n "), watch.UNREADABLE.empty)
	t.equal(why("Sure! Here is a note."), watch.UNREADABLE.no_learn)
	t.equal(why("note: something."), watch.UNREADABLE.no_learn)
	t.equal(why("learn:\ntag: Heads up"), watch.UNREADABLE.no_line)
	t.equal(why("learn: none\nbut also this"), watch.UNREADABLE.many_lines)
end)

-- Letters, digits, punctuation and characters of two, three and four
-- bytes, so a count of bytes and a count of characters differ.
local letter = g.sampled_from({ "a", "b", "X", "0", "9", " ", "`", ",", "_", "-", "é", "漢", "🦀" })

-- A line of `count` characters before its period, with no white space at
-- either end and never the word `none`.
local function body(draw, count)
	local list = draw(g.lists(letter, { min = count, max = count }))
	if list[1] == " " then
		list[1] = "a"
	end
	if list[#list] == " " then
		list[#list] = "a"
	end
	local text = table.concat(list)
	if string.lower(text) == "none" then
		text = "x" .. string.sub(text, 2)
	end
	return text
end

local word = g.from_regex("[A-Za-z`][A-Za-z0-9 `,._-]{0,40}[A-Za-z0-9`]")
local explanation = g.records({ title = word, bullets = g.lists(word, { min = 3, max = 5 }) })
local padding = g.sampled_from({ "", "\n", "  ", "\n\n", " \n " })

-- A note written the way the parser reads it, with or without an
-- explanation and wrapped in white space, reads back as written, up to a
-- line of MAX_LINE characters however many bytes they take.
t.property("a written note reads back", function(draw)
	local count = draw(g.one_of({
		g.sampled_from({ 1, 2, watch.MAX_LINE - 1, watch.MAX_LINE }),
		g.integers({ min = 1, max = watch.MAX_LINE }),
	}))
	local line = body(draw, count - 1) .. "."
	local tag = draw(g.sampled_from(watch.TAGS))
	local explain = draw(g.optional(explanation))
	local text = draw(padding) .. watch.write(tag, line, explain) .. draw(padding)
	t.equal(watch.parse(text), { kind = "note", tag = tag, line = line, explain = explain })
end, { cases = 200 })

-- A written note broken in one way is refused for that fault.
t.property("a note with one fault is refused for it", function(draw)
	local tag = draw(g.sampled_from(watch.TAGS))
	local line = body(draw, draw(g.integers({ min = 1, max = 60 }))) .. "."
	local fault = draw(g.sampled_from({ "no tag", "unknown tag", "no period", "too long", "bad explain" }))
	local text, want
	if fault == "no tag" then
		text, want = "learn: " .. line, watch.UNREADABLE.no_tag
	elseif fault == "unknown tag" then
		local other = draw(g.sampled_from({ "FYI", "heads up", "HEADS UP", "Heads-up", "You should know!", "Note" }))
		text = "learn: " .. line .. "\ntag: " .. other
		want = "the tag was `" .. other .. "`, not `You should know` or `Heads up`"
	elseif fault == "no period" then
		text, want = "learn: " .. body(draw, 20) .. "x\ntag: " .. tag, watch.UNREADABLE.no_period
	elseif fault == "too long" then
		local count = draw(g.integers({ min = watch.MAX_LINE + 1, max = watch.MAX_LINE + 40 }))
		text = "learn: " .. body(draw, count - 1) .. ".\ntag: " .. tag
		want = "the note's line has " .. count .. " characters, over " .. watch.MAX_LINE
	else
		local bullets = draw(g.integers({ min = 0, max = 7 }))
		t.assume(bullets < watch.FEWEST_BULLETS or bullets > watch.MOST_BULLETS)
		text = "learn: " .. line .. "\ntag: " .. tag .. "\nexplain:\n**A title**"
		for n = 1, bullets do
			text ..= "\n- bullet " .. n
		end
		want = watch.UNREADABLE.bad_explain
	end
	local reply, why = watch.parse(text)
	t.equal(reply, nil, text)
	t.equal(why, want, text)
end, { cases = 300 })

t.case("skips double from the third note up to the cap", function()
	local want = { [0] = 0, 0, 0, 1, 2, 4, 8, 16, 16, 16 }
	for typed_past = 0, 9 do
		t.equal(watch.skips(typed_past), want[typed_past], "after " .. typed_past)
	end
	t.equal(watch.skips(200), watch.MAX_SKIPS)
end)

-- However many notes were written past, the checks a long run makes are
-- the first due one, at step EVERY, then every skips + 1th due one.
t.property("checks come every sixth step, thinned by the back off", function(draw)
	local typed_past = draw(g.integers({ min = 0, max = 10 }))
	local last
	local checked = {}
	for steps = 1, 300 do
		if watch.due(steps, last, typed_past) then
			table.insert(checked, steps)
			last = steps
		end
	end
	local gap = watch.EVERY * (watch.skips(typed_past) + 1)
	t.equal(checked[1], watch.EVERY)
	for at = 2, #checked do
		t.equal(checked[at] - checked[at - 1], gap, "between checks")
	end
end)

t.case("a shrunk transcript starts the count again", function()
	t.equal(watch.due(6, 60, 10), true)
	t.equal(watch.due(6, 6, 0), false)
	t.equal(watch.due(7, nil, 0), false)
end)

t.property("the seen list keeps the last fifty", function(draw)
	local n = draw(g.integers({ min = 0, max = 120 }))
	local lines = {}
	for at = 1, n do
		table.insert(lines, "line " .. at)
	end
	local seen = watch.recent(lines)
	t.equal(#seen, math.min(n, watch.SEEN))
	if n > 0 then
		t.equal(seen[#seen], "line " .. n)
		t.equal(seen[1], "line " .. math.max(n - watch.SEEN + 1, 1))
	end
end)

-- Whatever the note says, the chat gets it whole after a fixed lead,
-- with no live mention or keyword in it: a word that starts with
-- `ultra` is broken after it, as each `ultra` it starts with again.
t.property("chat text has no live mentions or keywords", function(draw)
	local pieces = draw(g.lists(g.sampled_from({ "word", "@file", "ultrathink", "ULTRA", "ultraultra", "a@b", "\n", "\t", "é", "—" }), { max = 12 }))
	local line = table.concat(pieces, " ")
	local text = watch.chat_text(line)
	local lead = "Here is a note offered by a side agent:\n\n> "
	t.equal(string.sub(text, 1, #lead), lead)
	local quoted = string.sub(text, #lead + 1, -3)
	t.equal(string.find(quoted, "[\n\t]"), nil, "no control characters")
	t.equal(string.find(quoted, "@[^\u{200b}]") or string.match(quoted, "@$"), nil, "no live mention")
	local bare = string.gsub(quoted, "\u{200b}", "")
	t.equal(bare, (string.gsub(line, "[\n\t]", "")), "nothing else changes")
	for at in string.gmatch(string.lower(quoted), "()ultra") do
		t.equal(string.sub(quoted, at + 5, at + 7), "\u{200b}", "a broken keyword")
	end
end, { cases = 300 })

-- The notes a run keeps, whatever happens in whatever order: the band is
-- the newest note while it is new and written past fewer than
-- BAND_LIMIT times; answering one ends the back off; the seen list is
-- the last SEEN lines.
t.machine("the notes keep to their rules", {
	steps = 30,
	init = function()
		return { state = watch.state({}), step = 0, lines = {} }
	end,
	rules = {
		note = function(m, draw)
			m.step += watch.EVERY
			local line = "note " .. #m.lines .. "."
			table.insert(m.lines, line)
			watch.noted(m.state, m.step, { tag = draw(g.sampled_from(watch.TAGS)), line = line })
		end,
		drop = function(m)
			m.step += watch.EVERY
			watch.dropped(m.state, m.step)
		end,
		type_past = function(m)
			watch.typed_past(m.state)
		end,
		answer = function(m, draw)
			t.assume(#m.state.notes > 0)
			local note = m.state.notes[draw(g.integers({ min = 1, max = #m.state.notes }))]
			watch.answered(m.state, note.key, draw(g.sampled_from({ "learned", "knew", "chatted", "dismissed" })))
			t.equal(m.state.ignored, 0, "an answer ends the back off")
		end,
	},
	invariant = function(m)
		local newest = m.state.notes[#m.state.notes]
		local band = watch.band(m.state)
		if newest and newest.status == "new" and newest.typed_past < watch.BAND_LIMIT then
			t.equal(band, newest, "the band is the newest note")
		else
			t.equal(band, nil, "no band")
		end
		t.equal(watch.seen(m.state), watch.recent(m.lines))
		for at, note in ipairs(m.state.notes) do
			t.equal(note.key, "n" .. (at - 1))
		end
	end,
})
