-- A wake-up's name and the message a chat wakes on: every plain name is
-- one and one fault makes it none; a message reads back as written, and
-- text that does not start with a wake-up's header is no message.
local tau = require("tau")
local t = tau.test
local g = t.gen
local wake = require("wake")

-- A name a wake-up may have: a lower-case letter, then up to 39 lower-case
-- letters, digits, `-` and `_`, its shortest and longest drawn as often
-- as the rest.
local function plain_name(draw)
	local rest = draw(g.one_of({ g.sampled_from({ 0, 39 }), g.integers({ min = 0, max = 39 }) }))
	return draw(g.from_regex("[a-z]")) .. draw(g.from_regex("[a-z0-9_-]{" .. rest .. "}"))
end

t.property("names are short and plain", function(draw)
	local name = plain_name(draw)
	t.equal(wake.valid_name(name), true, name)
	local at = draw(g.integers({ min = 1, max = #name }))
	local fault = draw(g.integers({ min = 0, max = 3 }))
	local wrong
	if fault == 0 then
		wrong = ""
	elseif fault == 1 then
		wrong = name .. string.rep("a", 41 - #name)
	elseif fault == 2 then
		wrong = draw(g.sampled_from({ "0", "-", "_", "R" })) .. string.sub(name, 2)
	else
		local bad = draw(g.sampled_from({ "A", " ", ".", "/", "é" }))
		wrong = string.sub(name, 1, at - 1) .. bad .. string.sub(name, at + 1)
	end
	t.equal(wake.valid_name(wrong), false, wrong)
end)

-- A note as a chat writes one: lines of text, blank ones only between
-- them, none that looks like a wake-up's header.
local function note(draw)
	local line = g.from_regex("[A-Za-z0-9#?.,:'() -]{0,24}[A-Za-z0-9?.]")
	local lines = { draw(line) }
	for _, more in ipairs(draw(g.lists(g.one_of({ line, g.just("") }), { max = 3 }))) do
		table.insert(lines, more)
	end
	while lines[#lines] == "" do
		table.remove(lines)
	end
	return table.concat(lines, "\n")
end

local function woken(draw)
	return {
		name = draw(g.from_regex("[a-z][a-z0-9_-]{0,12}")),
		due = string.format(
			"2026-%02d-%02d %02d:%02d",
			draw(g.integers({ min = 1, max = 12 })),
			draw(g.integers({ min = 1, max = 28 })),
			draw(g.integers({ min = 0, max = 23 })),
			draw(g.integers({ min = 0, max = 59 }))
		),
		note = note(draw),
	}
end

t.property("a message reads back as written", function(draw)
	local all = {}
	for _ = 1, draw(g.integers({ min = 1, max = 4 })) do
		table.insert(all, woken(draw))
	end
	local text = wake.message(all)
	t.equal(string.sub(text, 1, #("[scheduled · " .. all[1].name .. " · ")), "[scheduled · " .. all[1].name .. " · ")
	t.equal(wake.parse(text), all, text)
	local before = draw(g.sampled_from({ "", "Did it merge?", " " }))
	t.equal(wake.parse(before .. "\n" .. text), nil)
	t.equal(wake.parse(before), nil)
end)
