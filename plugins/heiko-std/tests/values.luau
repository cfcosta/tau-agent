-- `heiko.values`: tables as values.
local heiko = require("heiko")
local t = heiko.test
local g = t.gen
local values = heiko.values

-- A value as JSON has them: numbers, text, booleans, lists and maps,
-- four deep at most.
local function value_of(draw, depth)
	local kind = draw(g.integers({ min = 1, max = depth > 0 and 5 or 3 }))
	if kind == 1 then
		return draw(g.integers({ min = -9, max = 9 }))
	elseif kind == 2 then
		return draw(g.from_regex("[a-c]{0,3}"))
	elseif kind == 3 then
		return draw(g.booleans())
	elseif kind == 4 then
		local list = array({})
		for _ = 1, draw(g.integers({ min = 0, max = 3 })) do
			table.insert(list, value_of(draw, depth - 1))
		end
		return list
	end
	local map = {}
	for _ = 1, draw(g.integers({ min = 0, max = 3 })) do
		map[draw(g.from_regex("[a-d]"))] = value_of(draw, depth - 1)
	end
	return map
end

t.case("count is how many keys", function()
	t.equal(values.count({}), 0)
	t.equal(values.count({ a = 1, b = 2 }), 2)
	t.equal(values.count({ "x", "y", "z" }), 3)
end)

t.case("a copy is apart from what it copied", function()
	local first = { list = array({ { n = 1 } }), name = "a" }
	local copy = values.copy(first)
	t.equal(copy, first)
	copy.list[1].n = 2
	copy.name = "b"
	t.equal(first, { list = array({ { n = 1 } }), name = "a" })
	-- A list stays one, empty too.
	t.equal(values.is_array(values.copy(array({}))), true)
	t.equal(values.copy(7), 7)
	t.equal(values.copy(nil), nil)
end)

t.case("same looks all the way down", function()
	t.equal(values.same({ a = { 1, 2 } }, { a = { 1, 2 } }), true)
	t.equal(values.same({ a = { 1, 2 } }, { a = { 1, 3 } }), false)
	t.equal(values.same({ a = 1 }, { a = 1, b = 2 }), false)
	t.equal(values.same({ a = 1, b = 2 }, { a = 1 }), false)
	t.equal(values.same(1, 1), true)
	t.equal(values.same("1", 1), false)
	t.equal(values.same({}, {}), true)
end)

t.case("a list is marked as one, or has only numbers for keys", function()
	t.equal(values.is_array(array({})), true)
	t.equal(values.is_array({ "a", "b" }), true)
	t.equal(values.is_array({}), false)
	t.equal(values.is_array({ a = 1 }), false)
	t.equal(values.is_array({ "a", b = 1 }), false)
	t.equal(values.is_array("ab"), false)
	t.equal(values.is_array(nil), false)
end)

t.case("contains is by equality, over a list", function()
	t.equal(values.contains({ "sat", "sun" }, "sun"), true)
	t.equal(values.contains({ "sat", "sun" }, "mon"), false)
	t.equal(values.contains(nil, "mon"), false)
end)

t.property("a copy is the same, and a change to it is not", function(draw)
	local value = value_of(draw, 4)
	local copy = values.copy(value)
	t.equal(values.same(value, copy), true)
	t.equal(values.same(copy, value), true)
	if type(copy) == "table" then
		copy["a key no value has"] = true
		t.equal(values.same(value, copy), false)
		t.equal(values.same(copy, value), false)
		t.equal(value["a key no value has"], nil, "what was copied is as it was")
	end
end)

t.property("count is one more for each new key", function(draw)
	local keys = draw(g.lists(g.from_regex("[a-z]{1,3}"), { min = 0, max = 12, unique = true }))
	local map = {}
	for n, key in ipairs(keys) do
		map[key] = n
		t.equal(values.count(map), n)
	end
end)
