local tau = require("tau")
local t = tau.test
local g = t.gen
local titles = require("titles")

t.case("asks the model, and keeps a clean title", function()
	local asked
	local run = t.run({
		tools = {
			infer = function(args)
				asked = args
				return { ok = true, value = { title = '  "Fix  the retry loop." ' } }
			end,
		},
	})
	t.equal(run:title({ task = "the retry loop gives up too soon", model = "m" }), "Fix the retry loop")
	t.equal(asked.effort, "low")
	t.equal(asked.schema, titles.SCHEMA)
	t.equal(string.find(asked.task, "the retry loop gives up too soon", 1, true) ~= nil, true)
end)

t.case("writes none when switched off", function()
	local run = t.run({ settings = { on = false } })
	t.equal(run:title({ task = "anything", model = "m" }), nil)
end)

t.case("an answer cleaned to nothing is no title", function()
	t.equal(titles.clean(" ... "), nil)
	t.equal(titles.clean("Why?!"), "Why")
	t.equal(utf8.len(titles.clean(string.rep("é", 50))), titles.MAX_CHARS)
end)

-- Characters of one to four bytes, and white space of both kinds.
local units = g.sampled_from({ "a", "Z", "é", "語", "🦀", " ", "\u{3000}", "\n" })

local function text_of(list)
	return table.concat(list)
end

-- A request is the instructions, then the most of the trimmed task that
-- fits the budget, cut between characters, whatever its length.
t.property("requests fit the budget", function(draw)
	local task = text_of(draw(g.lists(units, { max = 1000 })))
	local request = titles.request(task)
	t.equal(#request <= titles.PROMPT_MAX_BYTES, true, "within the budget")
	t.equal(utf8.len(request) ~= nil, true, "whole characters")
	local prefix = titles.request("")
	t.equal(string.sub(request, 1, #prefix), prefix)
	local kept = string.sub(request, #prefix + 1)
	local trimmed = titles.trim(task)
	t.equal(string.sub(trimmed, 1, #kept), kept, "a prefix of the task")
	if #kept < #trimmed then
		local next = utf8.char(utf8.codepoint(trimmed, #kept + 1))
		t.equal(#prefix + #kept + #next > titles.PROMPT_MAX_BYTES, true, "the next would not fit")
	end
end, { cases = 300 })

-- A cleaned title is at most MAX_CHARS, starts with no white space, and
-- has none twice in a row.
t.property("a clean title is short and single spaced", function(draw)
	local raw = text_of(draw(g.lists(g.one_of({ units, g.sampled_from({ ".", "?", "!", '"', "“", "’", "`" }) }), { max = 60 })))
	local clean = titles.clean(raw)
	if clean ~= nil then
		t.equal(utf8.len(clean) <= titles.MAX_CHARS, true, clean)
		t.equal(titles.trim(clean) == clean or string.match(clean, "%s$") ~= nil, true, "no white space before it")
		t.equal(string.find(clean, "  ", 1, true), nil, "no runs of spaces")
	end
end, { cases = 300 })
