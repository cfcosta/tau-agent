-- How `/goal` reads: a command written out with its limits reads back as
-- itself, a budget is a finite amount of at least zero, and the
-- condition a command set reads back from the input the model gets;
-- what is not a command reads as none.
local tau = require("tau")
local t = tau.test
local g = t.gen
local goal = require("goal")

-- White space between the words of a command, as a person may type it.
local GAP = g.sampled_from({ " ", "  ", "\t", "\n", " \n " })

-- A command written out: `/goal`, the limits it names, in the order
-- drawn, and the condition's words, with drawn white space between.
-- Gives the text and the command it should read as.
local function written(draw)
	local continuations = draw(g.optional(g.integers({ min = 0, max = 4294967295 })))
	local budget = draw(g.optional(g.floats({ min = 0, max = 1e12 })))
	local dollar = draw(g.booleans())
	local budget_first = draw(g.booleans())
	-- The first word cannot be a limit, so every shrunk command has a
	-- condition.
	local words = {
		draw(g.one_of({ g.from_regex("[a-z0-9$.,:][a-z0-9$.,:-]{0,7}"), g.just("clear") })),
	}
	for _, word in
		ipairs(draw(g.lists(
			g.one_of({ g.from_regex("[a-z0-9$.,:-]{1,8}"), g.just("clear"), g.just("--budget") }),
			{ max = 4 }
		)))
	do
		table.insert(words, word)
	end
	local limits = {}
	if continuations ~= nil then
		table.insert(limits, "--continuations" .. draw(GAP) .. continuations)
	end
	if budget ~= nil then
		table.insert(limits, "--budget" .. draw(GAP) .. (dollar and "$" or "") .. string.format("%.17g", budget))
	end
	if budget_first and #limits == 2 then
		limits = { limits[2], limits[1] }
	end
	local text = draw(GAP) .. "/goal"
	for _, part in ipairs(limits) do
		text ..= draw(GAP) .. part
	end
	for _, word in ipairs(words) do
		text ..= draw(GAP) .. word
	end
	text ..= draw(GAP)
	local condition = table.concat(words, " ")
	if condition == "clear" then
		return text, { kind = "clear" }
	end
	return text,
		{
			kind = "set",
			condition = condition,
			continuations = continuations or goal.DEFAULT_CONTINUATIONS,
			budget = budget or goal.DEFAULT_BUDGET,
		}
end

t.property("a written command reads as itself", function(draw)
	local text, command = written(draw)
	t.equal(goal.parse(text), command, text)
	if command.kind == "set" then
		t.equal(goal.set_message(goal.set_input(command.condition)), command.condition)
	end
end)

-- A `--budget` is taken exactly when it is a finite number of at least
-- zero: `nan`, `inf`, an overflow and negative amounts make the input
-- not a command.
t.property("a budget is a finite amount not below zero", function(draw)
	local cases = {
		{ "nan", nil },
		{ "NaN", nil },
		{ "inf", nil },
		{ "-inf", nil },
		{ "infinity", nil },
		{ "-3", nil },
		{ "-0.5", nil },
		{ "-0", 0 },
		{ "1e400", nil },
		{ "0", 0 },
		{ "0.25", 0.25 },
		{ "1e3", 1000 },
		{ "0x10", nil },
		{ "5.", 5 },
		{ ".5", 0.5 },
	}
	local drawn = draw(g.one_of({
		g.floats({ min = -1e9, max = 1e9 }):map(function(x)
			return { string.format("%.17g", x), x >= 0 and x or nil }
		end),
		g.sampled_from(cases),
	}))
	local amount, expected = drawn[1], drawn[2]
	local command = goal.parse("/goal --budget " .. amount .. " tests pass")
	if expected == nil then
		t.equal(command, nil, amount)
	else
		t.equal(command and command.budget, expected, amount)
	end
end)

-- Any condition reads back from the input the model gets for it: one
-- that would not read back written as is is quoted.
t.property("any condition reads back from the model's input", function(draw)
	local condition = draw(g.from_regex("[a-zé][^\\x00]{0,39}"))
	t.equal(goal.set_message(goal.set_input(condition)), condition)
	for _, collision in
		ipairs({ "clear", "--budget 5 tests pass", "--continuations 3 x", "tests\npass", '"quoted"', "tests  pass", " tests pass" })
	do
		t.equal(goal.set_message(goal.set_input(collision)), collision, collision)
	end
end)

-- A condition, and whether the model's input may write it as is: a few
-- words joined by single spaces, or those words broken in one of the
-- ways that would read back otherwise.
local function condition(draw)
	local words = { draw(g.from_regex([[[a-z0-9é,.:'-][a-z0-9é,.:'"\\-]{0,6}]])) }
	for _, word in ipairs(draw(g.lists(g.from_regex([[[a-z0-9é,.:'"\\-]{1,7}]]), { max = 3 }))) do
		table.insert(words, word)
	end
	local joined = table.concat(words, " ")
	local way = draw(g.integers({ min = 0, max = 9 }))
	if way == 0 then
		return " " .. joined, false
	elseif way == 1 then
		return joined .. " ", false
	elseif way == 2 and #words > 1 then
		return table.concat(words, "  "), false
	elseif way == 3 and #words > 1 then
		return table.concat(words, draw(g.sampled_from({ "\n", "\r\n", "\t", "\u{a0}" }))), false
	elseif way == 4 then
		return draw(g.sampled_from({ "--budget 5", "--continuations 3", "--budget $0.5" })) .. " " .. joined, false
	elseif way == 5 then
		return "clear", false
	elseif way == 6 then
		return goal.quote(joined), false
	end
	return joined, joined ~= "clear"
end

-- The model's input writes a condition as `/goal <condition>` exactly
-- when that reads back as the condition, and otherwise as one quoted
-- string; either way it reads back exactly.
t.property("a condition is quoted exactly when it would not read back", function(draw)
	local text, plain = condition(draw)
	local first = plain and ("/goal " .. text) or ("/goal " .. goal.quote(text))
	local input = goal.set_input(text)
	t.equal(string.match(input, "^[^\n]*"), first, text)
	t.equal(goal.set_message(input), text)
end)

t.case("conditions are quoted as they always were", function()
	for _, case in
		ipairs({
			{ "cargo test passes", "/goal cargo test passes" },
			{ '"a" and "b"', '/goal "a" and "b"' },
			{ '"a\\b"', '/goal "a\\b"' },
			{ "clear", '/goal "clear"' },
			{ "--budget 5 tests pass", '/goal "--budget 5 tests pass"' },
			{ "--continuations 3 x", '/goal "--continuations 3 x"' },
			{ "tests\npass", '/goal "tests\\npass"' },
			{ "a\r\nb", '/goal "a\\r\\nb"' },
			{ "tests  pass", '/goal "tests  pass"' },
			{ " tests pass", '/goal " tests pass"' },
			{ '"quoted"', '/goal "\\"quoted\\""' },
			{ '"a\\\\b"', '/goal "\\"a\\\\\\\\b\\""' },
		})
	do
		local input = goal.set_input(case[1])
		t.equal(string.match(input, "^[^\n]*"), case[2], case[1])
		t.equal(goal.set_message(input), case[1])
	end
end)

-- A quoted condition is read exactly, after any limits; one that is not
-- a whole quoted string reads as words, quotes included.
t.case("a quoted condition reads exactly", function()
	t.equal(
		goal.parse('/goal --budget 3 "clear"'),
		{ kind = "set", condition = "clear", continuations = goal.DEFAULT_CONTINUATIONS, budget = 3 }
	)
	t.equal(goal.parse('/goal "a" and  "b"'), {
		kind = "set",
		condition = '"a" and "b"',
		continuations = goal.DEFAULT_CONTINUATIONS,
		budget = goal.DEFAULT_BUDGET,
	})
	t.equal(goal.parse('/goal "  "'), nil)
end)

-- Input that is not a `/goal` command reads as none: no condition, a
-- longer word than `/goal`, the command not at the start, or a limit
-- with no amount. A message that is not a goal's input names no
-- condition.
t.case("what is not a goal command is not read", function()
	for _, text in ipairs({ "/goal", "/goal   ", "/goals x", "fix /goal x", "/goal --budget" }) do
		t.equal(goal.parse(text), nil, text)
	end
	t.equal(goal.set_message("fix the tests"), nil)
end)
