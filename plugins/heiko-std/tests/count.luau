-- `heiko.count`: numbers as the interface says them.
local heiko = require("heiko")
local t = heiko.test
local g = t.gen
local count = heiko.count

t.case("one of a thing, or many", function()
	t.equal(count.plural(1, "note"), "1 note")
	t.equal(count.plural(0, "note"), "0 notes")
	t.equal(count.plural(3, "note"), "3 notes")
	t.equal(count.plural(2, "reply", "replies"), "2 replies")
	t.equal(count.plural(1, "reply", "replies"), "1 reply")
end)

t.case("tokens are short, and rounded down", function()
	t.equal(count.tokens(0), "0")
	t.equal(count.tokens(999), "999")
	t.equal(count.tokens(1000), "1k")
	t.equal(count.tokens(184999), "184k")
	t.equal(count.tokens(999999), "999k")
	t.equal(count.tokens(1000000), "1.0M")
	t.equal(count.tokens(1250000), "1.2M")
	t.equal(count.tokens(1999999), "1.9M")
	t.equal(count.tokens(12.7), "12")
end)

t.property("a count of tokens never reads as more than it is", function(draw)
	local n = draw(g.integers({ min = 0, max = 500000000 }))
	local said = count.tokens(n)
	local number, unit = string.match(said, "^([%d.]+)([kM]?)$")
	t.equal(number ~= nil, true, said)
	local scale = unit == "k" and 1000 or unit == "M" and 1000000 or 1
	local read = tonumber(number) * scale
	t.equal(read <= n, true, said .. " for " .. n)
	-- And is off by less than its last digit's worth.
	local step = unit == "M" and 100000 or scale
	t.equal(n - read < step, true, said .. " for " .. n)
end)
