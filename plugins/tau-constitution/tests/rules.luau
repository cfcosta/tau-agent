-- Rules: a stored constitution that does not check out says what is
-- wrong with it; adding, replacing and removing rules does what the same
-- edits do to a plain list; and what a rule is shown of a call comes
-- from under the field it names, however deep.
local tau = require("tau")
local t = tau.test
local g = t.gen
local rules = require("rules")

local function rule(id, text, on, review, block)
	return { id = id, text = text, on = on, review = review, block = block }
end

local function stored(list, on_error)
	return { on_error = on_error or "allow", max_holds = 3, rules = list }
end

t.case("bad constitutions say what is wrong", function()
	local function bad(list, on_error)
		local constitution, why = rules.from_stored(stored(list, on_error))
		t.equal(constitution, nil)
		return why
	end
	t.equal(bad({ rule("A", "t", {}, 0.5, 0.8) }), rules.errors.nowhere("A"))
	t.equal(bad({ rule("A", "t", { "exec_command" }, 0.5, 0.8) }), rules.errors.target("A", rules.errors.no_target("exec_command")))
	t.equal(bad({ rule("A", "t", { "a.b" }, 0.9, 0.5) }), "Rule A: review (0.9) is above block (0.5)")
	t.equal(bad({ rule("A", "t", { "a.b" }, 0.5, 0.8), rule("A", "u", { "a.b" }, 0.5, 0.8) }), "Two rules are called A")
	t.equal(bad({}, "maybe"), "`maybe` is not allow or block")
	t.equal(bad({ rule(" ", "t", { "a.b" }, 0.5, 0.8) }), "A rule has no id")
	t.equal(bad({ rule("A", " \u{3000}", { "a.b" }, 0.5, 0.8) }), "Rule A has no text")
	t.equal(bad({ rule("A", "t", { "a.b" }, -0.1, 0.8) }), "Rule A: review and block are probabilities, 0 to 1")
	-- None stored is no rules.
	t.equal(rules.from_stored(nil), rules.default())
end)

t.case("targets read as Rust read them", function()
	t.equal(rules.target(" Final Answer "), { final = true })
	t.equal(rules.target("a.b.c"), { tool = "a", field = "b.c" })
	t.equal(rules.target(" exec_command.cmd\u{a0}"), { tool = "exec_command", field = "cmd" })
	for _, bad in ipairs({ "exec_command", ".x", "x.", "" }) do
		t.equal(rules.target(bad), nil, bad)
	end
	t.equal(rules.float(0.1), "0.1")
	t.equal(rules.float(1), "1")
	t.equal(rules.float(1e-7), "0.0000001")
	t.equal(rules.float(-0.0), "-0")
end)

-- Targets as written in the UI: good ones, spaced ones, and ones that
-- name nothing.
local TARGETS = { "edit.newText", "write.content", " exec_command.cmd ", "a.b.c", "final answer", "Final Answer", "exec_command", ".x", "x.", "" }

-- The label a target reads back as, when it names one.
local function label(target)
	target = rules.trim(target)
	if string.lower(target) == "final answer" then
		return "final answer"
	end
	local tool, field = string.match(target, "^([^%.]*)%.(.*)$")
	if tool == nil or tool == "" or field == "" then
		return nil
	end
	return target
end

local function valid_targets()
	local list = {}
	for _, target in ipairs(TARGETS) do
		if label(target) then
			table.insert(list, target)
		end
	end
	return list
end

-- What an add or replace is given: mostly a valid rule, so edits go
-- through; otherwise anything.
local edit = g.composite(function(draw)
	if draw(g.booleans({ p = 0.7 })) then
		local review = draw(g.floats({ min = 0, max = 1 }))
		return {
			text = draw(g.sampled_from({ "Never force-push.", "  No unwrap.  " })),
			targets = draw(g.lists(g.sampled_from(valid_targets()), { min = 1, max = 3 })),
			review = review,
			block = draw(g.floats({ min = review, max = 1 })),
		}
	end
	local threshold = g.one_of({ g.floats({ min = 0, max = 1 }), g.sampled_from({ 0, 1, -0.1, 1.5, 0 / 0 }) })
	return {
		text = draw(g.sampled_from({ "Never force-push.", "   ", "" })),
		targets = draw(g.lists(g.sampled_from(TARGETS), { max = 3 })),
		review = draw(threshold),
		block = draw(threshold),
	}
end)

-- The rule an edit makes under `id`, if it is a valid one.
local function model_rule(e, id)
	local labels = {}
	for _, target in ipairs(e.targets) do
		local l = label(target)
		if l == nil then
			return nil
		end
		table.insert(labels, l)
	end
	local function p(x)
		return x >= 0 and x <= 1
	end
	if #labels == 0 or rules.trim(e.text) == "" or not p(e.review) or not p(e.block) or e.review > e.block then
		return nil
	end
	return { id = id, text = rules.trim(e.text), on = labels, review = e.review, block = e.block }
end

local function some_id(model, draw)
	local ids = { "R1", "R2", "R9" }
	for _, r in ipairs(model.list) do
		table.insert(ids, r.id)
	end
	return draw(g.sampled_from(ids))
end

local function at(model, id)
	for n, r in ipairs(model.list) do
		if r.id == id then
			return n
		end
	end
	return nil
end

t.machine("rules are edited like a list", {
	init = function()
		return { constitution = rules.default(), list = {} }
	end,
	rules = {
		-- Adding takes the lowest `R{n}` no rule has, at the end; a rule
		-- that is not valid is refused and changes nothing.
		add = function(model, draw)
			local e = draw(edit)
			local n = 1
			while at(model, "R" .. n) do
				n += 1
			end
			local added = rules.add(model.constitution, e.text, e.targets, e.review, e.block)
			local made = model_rule(e, "R" .. n)
			if made then
				t.equal(added, "R" .. n)
				table.insert(model.list, made)
			else
				t.equal(added, nil)
			end
		end,
		-- Replacing rewrites a rule in place, keeping its id; an unknown id
		-- or a rule that is not valid is refused and changes nothing.
		replace = function(model, draw)
			local id = some_id(model, draw)
			local e = draw(edit)
			local replaced = rules.replace(model.constitution, id, e.text, e.targets, e.review, e.block)
			local n, made = at(model, id), model_rule(e, id)
			if n and made then
				t.equal(replaced, true)
				model.list[n] = made
			else
				t.equal(replaced, nil)
			end
		end,
		-- Removing takes out the rule with that id, and says whether there
		-- was one.
		remove = function(model, draw)
			local id = some_id(model, draw)
			local n = at(model, id)
			t.equal(rules.remove(model.constitution, id), n ~= nil)
			if n then
				table.remove(model.list, n)
			end
		end,
	},
	-- The rules are the model's, in its order, with unique ids and review
	-- at most block, and they come back from the store's shape.
	invariant = function(model)
		t.equal(model.constitution.rules, model.list)
		local seen = {}
		for _, r in ipairs(model.constitution.rules) do
			t.equal(r.review <= r.block, true)
			t.equal(seen[r.id], nil)
			seen[r.id] = true
		end
		local back = rules.from_stored(json.decode(json.encode(model.constitution)))
		t.equal(back, model.constitution)
	end,
	steps = 30,
}, { cases = 100 })

-- Field names a call's arguments use, so rules find some.
local KEYS = { "k", "x", "path", "edits" }

local leaf = g.one_of({
	g.integers({ min = -5, max = 5 }),
	g.text({ max = 4 }),
	g.booleans(),
	g.just(json.null),
})

-- Arguments: objects and arrays up to `depth` deep, keyed from `KEYS`,
-- with JSON at the leaves.
local function arguments(depth)
	return g.composite(function(draw)
		local kind = depth == 0 and 0 or draw(g.integers({ min = 0, max = 2 }))
		if kind == 0 then
			return draw(leaf)
		elseif kind == 1 then
			return draw(g.dicts(g.sampled_from(KEYS), arguments(depth - 1), { max = 3 }))
		end
		return draw(g.lists(arguments(depth - 1), { max = 3 }))
	end)
end

-- Every value under `key` anywhere in `value`, nested ones included, in
-- the order the rules walk it.
local function everything_under(value, key, found)
	if type(value) ~= "table" then
		return
	end
	local names = {}
	for name in pairs(value) do
		table.insert(names, name)
	end
	table.sort(names, function(a, b)
		return tostring(a) < tostring(b)
	end)
	if value[1] ~= nil then
		names = {}
		for n = 1, #value do
			names[n] = n
		end
	end
	for _, name in ipairs(names) do
		if name == key then
			table.insert(found, value[name])
		end
		everything_under(value[name], key, found)
	end
end

-- One rule on `tool.k`, one on another tool, one on the final answer.
local function on_k()
	local constitution = rules.default()
	for _, on in ipairs({ "tool.k", "other.k", "final answer" }) do
		rules.add(constitution, "A rule.", { on }, 0.5, 0.8)
	end
	return constitution
end

local function contains(list, value)
	for _, item in ipairs(list) do
		if json.encode(item) == json.encode(value) then
			return true
		end
	end
	return false
end

t.property("a rule is shown what is under its field", function(draw)
	-- Half the time, the edits of a call that makes several: the field in
	-- each of a list of objects, as `edit.newText` is.
	local args, edits
	if draw(g.booleans()) then
		args = draw(arguments(3))
	else
		edits = draw(g.lists(leaf, { min = 2, max = 4 }))
		local list = array({})
		for _, e in ipairs(edits) do
			table.insert(list, { x = 1, k = e })
		end
		args = { path = "src/lib.rs", edits = list }
	end
	local constitution = on_k()
	local answer = rules.for_final_answer(constitution)
	t.equal(#answer, 1)
	t.equal(answer[1].id, "R3", "only the rule on the final answer is about it")
	local found = rules.for_call(constitution, "tool", args)
	local under = {}
	everything_under(args, "k", under)
	t.equal(#found, #under > 0 and 1 or 0)
	if #found == 0 then
		return
	end
	t.equal(found[1].rule.id, "R1")
	t.equal(found[1].names, { "k" })
	local shown = found[1].fields.k
	if edits then
		-- Each edit's field, every one of them, in order.
		t.equal(shown, edits)
	elseif contains(under, shown) then
		t.note("one value")
	else
		t.note("several values")
		t.equal(type(shown), "table")
		t.equal(#shown >= 1, true)
		-- Each was under the field, in the order the rules walk them.
		local rest = 1
		for _, value in ipairs(shown) do
			while rest <= #under and json.encode(under[rest]) ~= json.encode(value) do
				rest += 1
			end
			t.equal(rest <= #under, true, "in order under k")
			rest += 1
		end
	end
end, { cases = 200 })

-- What a rule is shown, by rule.
local function shown_by(constitution, args)
	local out = {}
	for _, found in ipairs(rules.for_call(constitution, "tool", args)) do
		out[found.rule.id] = json.encode(found.fields)
	end
	return out
end

t.property("wrapping the arguments shows the same", function(draw)
	local args = draw(arguments(3))
	local constitution = on_k()
	local direct = shown_by(constitution, args)
	t.equal(shown_by(constitution, { x = args }), direct)
	t.equal(shown_by(constitution, array({ args })), direct)
end, { cases = 200 })

t.property("a top-level field is shown as it is", function(draw)
	local inner = draw(arguments(3))
	local found = rules.for_call(on_k(), "tool", { k = inner, path = "src/lib.rs" })
	t.equal(#found, 1)
	t.equal(found[1].fields.k, inner)
end, { cases = 200 })
