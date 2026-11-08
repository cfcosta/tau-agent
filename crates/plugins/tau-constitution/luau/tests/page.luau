-- The rules page: rules are written, edited and tried in the editor,
-- unreadable rules are removed only once confirmed, what waits for a
-- person leaves the queue once it looks fine, and the page says when
-- nothing can be checked.
local tau = require("tau")
local t = tau.test
local g = t.gen
local rules = require("rules")
local page = require("page")

local function has(text, part)
	return string.find(text, part, 1, true) ~= nil
end

-- What a tree says, as text.
local function said(tree)
	return json.encode(tree)
end

local function page_run(store, jev)
	return t.run({ store = store or {}, jev = jev })
end

t.property("holds stay in bounds", function(draw)
	local holds = draw(g.integers({ min = 0, max = page.MAX_HOLDS }))
	local delta = draw(g.integers({ min = -20, max = 20 }))
	local next = page.next_holds(holds, delta)
	t.equal(next, math.clamp(holds + delta, 0, page.MAX_HOLDS))
end, { cases = 200 })

t.property("a step keeps review at most block, to two places", function(draw)
	local draft = page.draft(nil)
	for _ = 1, draw(g.integers({ min = 0, max = 30 })) do
		local mark = draw(g.sampled_from({ "review", "block" }))
		page.nudge(draft, mark, draw(g.sampled_from({ -0.05, 0.05, -0.35 })))
		t.equal(draft.review <= draft.block, true)
		t.equal(draft.review >= 0 and draft.block <= 1, true)
		t.equal(page.round(draft.review), draft.review)
	end
end, { cases = 100 })

t.case("rules are written and edited in the editor", function()
	local r = page_run()
	r:action("open", {})
	r:action("write", { text = "Never delete an index." })
	-- Nothing picked: nothing is saved, and the editor says why.
	r:action("save", {})
	t.equal(r.store.constitution, nil)
	t.equal(r.store.editor.tried_to_save, true)
	t.equal(page.problem(r.store.editor), "Pick at least one place, or Jev has nothing to check.")
	t.equal(has(said(r:page("rules", { repo = "docbert" })), "Pick at least one place"), true)
	-- Places are picked, and any tool's field can be added by name.
	r:action("place", { place = "exec_command.cmd" })
	r:action("add_place", { text = "not a field" })
	t.equal(r.store.editor.bad_place, "not a field")
	t.equal(r.store.editor.places, { "exec_command.cmd" })
	r:action("add_place", { text = " grep.pattern " })
	t.equal(r.store.editor.bad_place, nil)
	t.equal(r.store.editor.places, { "exec_command.cmd", "grep.pattern" })
	-- Presets, then a step: review never passes block.
	r:action("preset", { preset = "strict" })
	t.equal(page.preset(r.store.editor).name, "strict")
	r:action("nudge", { mark = "block", delta = 0.05 })
	t.equal(page.preset(r.store.editor), nil)
	for _ = 1, 20 do
		r:action("nudge", { mark = "review", delta = 0.05 })
	end
	t.equal({ r.store.editor.review, r.store.editor.block }, { 0.65, 0.65 })
	r:action("nudge", { mark = "review", delta = -0.35 })
	r:action("save", {})
	t.equal(r.store.editor, nil, "the editor closes")
	t.equal(r.store.constitution.rules, {
		{ id = "R1", text = "Never delete an index.", on = { "exec_command.cmd", "grep.pattern" }, review = 0.3, block = 0.65 },
	})
	-- Editing opens with the rule as it is, and saves over it.
	r:action("open", { rule = "R1" })
	t.equal(r.store.editor.editing, "R1")
	t.equal(r.store.editor.text, "Never delete an index.")
	t.equal(r.store.editor.places, { "exec_command.cmd", "grep.pattern" })
	r:action("place", { place = "grep.pattern" })
	r:action("save", {})
	t.equal(r.store.constitution.rules[1].on, { "exec_command.cmd" })
	t.equal(r.store.constitution.rules[1].id, "R1")
	r:action("remove", { id = "R1" })
	t.equal(r.store.constitution.rules, {})
end)

t.case("Enter keeps what was typed, and Save saves it", function()
	local r = page_run()
	r:action("open", {})
	r:action("write", { text = "Never delete an index." })
	t.equal(r.store.constitution, nil, "Enter does not save")
	t.equal(r.store.editor.text, "Never delete an index.")
	-- What it keeps shows, to write again; blank changes nothing.
	r:action("write", { text = "  " })
	t.equal(has(said(r:page("rules", { repo = "docbert" })), "Never delete an index."), true)
	r:action("place", { place = "exec_command.cmd" })
	r:action("save", {})
	t.equal(r.store.constitution.rules[1], {
		id = "R1",
		text = "Never delete an index.",
		on = { "exec_command.cmd" },
		review = 0.3,
		block = 0.8,
	})
	-- Without the rule written, nothing is saved.
	r:action("open", {})
	r:action("place", { place = "exec_command.cmd" })
	r:action("save", {})
	t.equal(#r.store.constitution.rules, 1)
	t.equal(has(page.problem(r.store.editor), "Write the rule"), true)
end)

-- A Jev scoring each question 0.9, keeping each request.
local function jev()
	local asked = {}
	return {
		ask = function(request)
			table.insert(asked, request)
			local answers = {}
			for id in pairs(request.questions) do
				answers[id] = { probability = 0.9 }
			end
			return answers, 0.001
		end,
	}, asked
end

t.case("a rule is tried on what a check would show", function()
	local fake, asked = jev()
	local samples = {
		calls = {
			{ tool = "write", args = { path = "a", content = "x.unwrap()" } },
			-- Not about write.content: skipped.
			{ tool = "exec_command", args = { cmd = "ls" } },
			{ tool = "write", args = { path = "b", content = "Ok(x)" } },
		},
		answers = { "Done." },
	}
	local draft = page.draft(nil)
	draft.text = "No unwrap."
	draft.places = { "write.content" }
	local calls, answers = page.trial_samples(draft, samples)
	t.equal(#calls, 2)
	t.equal(answers, {})
	local trials, cost = page.try(fake, draft, calls, answers)
	t.equal(trials, { { tool = "write", shown = "x.unwrap()", score = 0.9 }, { tool = "write", shown = "Ok(x)", score = 0.9 } })
	t.equal(cost > 0, true)
	-- Jev saw what the check sees: the tool and the field, nothing else.
	t.equal(#asked, 2)
	t.equal(asked[1].state, { tool = "write", arguments = { content = "x.unwrap()" } })
	-- A final-answer rule tries the answers.
	draft.places = { "final answer" }
	calls, answers = page.trial_samples(draft, samples)
	trials = page.try(fake, draft, calls, answers)
	t.equal(trials, { { shown = "Done.", score = 0.9 } })
	t.equal(asked[3].state, { final_answer = "Done." })
	-- A place that names nothing is refused.
	draft.places = { "nowhere" }
	t.equal({ page.try(fake, draft, {}, {}) }, { nil, rules.errors.no_target("nowhere") })
end)

t.case("a rule is tried and says what it would do", function()
	local fake = jev()
	local store = { samples = { calls = { { tool = "exec_command", args = { cmd = "psql prod" } } }, answers = {} } }
	local r = page_run(store, fake)
	r:action("open", {})
	r:action("write", { text = "Never touch the production database." })
	-- Nothing to check yet: nothing is tried.
	r:action("try", {})
	t.equal(r.store.trying, nil)
	t.equal(r.store.editor.tried_to_save, true)
	r:action("place", { place = "exec_command.cmd" })
	r:action("try", {})
	t.equal(r.store.trying.trials, { { tool = "exec_command", shown = "psql prod", score = 0.9 } })
	local drawn = said(r:page("rules", { repo = "tau-agent" }))
	t.equal(has(drawn, "0.90 would block"), true, drawn)
	-- Changing where the rule applies drops what trying it gave.
	r:action("place", { place = "exec_command.cmd" })
	t.equal(r.store.trying, nil)
	-- Without a key, it says so.
	local keyless = page_run({})
	keyless:action("open", {})
	keyless:action("write", { text = "Never touch the production database." })
	keyless:action("place", { place = "exec_command.cmd" })
	keyless:action("try", {})
	t.equal(has(keyless.store.trying.failed, "TypeSafe key"), true)
end)

t.case("rules that cannot be read are removed only once confirmed", function()
	local broken = { on_error = "allow", max_holds = 3, rules = { { id = "R1", text = "Name the tests.", on = { "final answer" }, review = 0.9, block = 0.2 } } }
	local r = page_run({ constitution = broken }, jev())
	local drawn = said(r:page("rules", { repo = "tau-agent" }))
	t.equal(has(drawn, "can't be read, so runs in tau-agent fail at start"), true, drawn)
	t.equal(has(drawn, "Rule R1: review (0.9) is above block (0.2)"), true)
	-- No edit gets past rules that cannot be read.
	r:action("open", {})
	r:action("write", { text = "No unwrap." })
	r:action("place", { place = "write.content" })
	t.equal(pcall(r.action, r, "save", {}), false)
	t.equal(r.store.constitution, broken)
	r:action("close", {})
	r:action("ask_reset", { yes = true })
	t.equal(r.store.resetting, true)
	t.equal(has(said(r:page("rules", { repo = "tau-agent" })), "This deletes them for good"), true)
	r:action("ask_reset", { yes = false })
	t.equal(r.store.resetting, nil)
	t.equal(r.store.constitution, broken, "asking is not removing")
	r:action("ask_reset", { yes = true })
	r:action("reset", {})
	t.equal(r.store.resetting, nil)
	t.equal(r.store.constitution, rules.default())
	-- Settings are saved as set.
	r:action("settings", { blocks_unchecked = true, max_holds = 4 })
	t.equal({ r.store.constitution.on_error, r.store.constitution.max_holds }, { "block", 4 })
	-- Without a key, the banner says runs fail once there is one.
	local keyless = page_run({ constitution = broken })
	t.equal(has(said(keyless:page("rules", { repo = "tau-agent" })), "once a TypeSafe key is added"), true)
end)

t.case("what waits for a person, and what was handled", function()
	local stored = {
		on_error = "allow",
		max_holds = 3,
		rules = {
			{ id = "R1", text = "No rm.", on = { "exec_command.cmd" }, review = 0.3, block = 0.8 },
			{ id = "R2", text = "No unwrap.", on = { "write.content" }, review = 0.3, block = 0.8 },
			{ id = "R6", text = "Name the tests.", on = { "final answer" }, review = 0.4, block = 0.75 },
		},
	}
	local scores = { 0.95, 0.55, 0.5 }
	local fake = {
		ask = function(request)
			local p = table.remove(scores, 1)
			local answers = {}
			for id in pairs(request.questions) do
				answers[id] = { probability = p }
			end
			return answers, 0.001
		end,
	}
	local r = t.run({ id = "a", jev = fake, store = { constitution = stored } })
	r:start({ input = "clean up\nand more" })
	r:before_tool({ id = "c1", name = "exec_command", args = { cmd = "rm -rf /" } })
	r:before_tool({ id = "c2", name = "write", args = { path = "a.rs", content = "x.unwrap()" } })
	r:before_stop({ text = "Done.", turn = 1, reason = "stop" })
	local waiting = {}
	for key, item in pairs(r.store) do
		if string.sub(key, 1, 7) == "review/" then
			table.insert(waiting, item)
		end
	end
	table.sort(waiting, function(a, b)
		return a.id < b.id
	end)
	t.equal(#waiting, 2)
	t.equal({ waiting[1].tool, waiting[1].rule, waiting[1].shown, waiting[1].title }, { "write", "R2", "x.unwrap()", "clean up" })
	t.equal({ waiting[2].tool, waiting[2].rule, waiting[2].shown }, { nil, "R6", "Done." })
	t.equal(r:repo_data().nav.badge, "2")
	local drawn = said(r:page("rules", { repo = "tau-agent" }))
	t.equal(has(drawn, "Review · 2") and has(drawn, "Waiting for you · 2"), true, drawn)
	t.equal(has(drawn, "1 blocked") and has(drawn, "2 flagged"), true, drawn)
	t.equal(has(drawn, "last: rm -rf /"), true, drawn)
	-- Looks fine: off the queue, and onto what was handled.
	r:action("fine", { id = waiting[2].id })
	t.equal(r.store[waiting[2].id], nil)
	t.equal(r:repo_data().nav.badge, "1")
	local fine = r.store.handled[#r.store.handled]
	t.equal({ fine.what, fine.shown, fine.rule }, { "fine", "final answer: Done.", "R6" })
	t.equal(r.store.handled[1].what, "blocked")
	-- Marking it again changes nothing.
	r:action("fine", { id = waiting[2].id })
	t.equal(#r.store.handled, 2)
end)

t.case("the page says what there is", function()
	-- No rules, no key: how to write one, and where to add a key.
	local empty = said(page_run({}):page("rules", { repo = "docbert" }))
	t.equal(has(empty, "No rules for docbert") and has(empty, "Add TypeSafe key"), true, empty)
	t.equal(has(empty, "Add a rule"), false, "nothing to count yet")
	-- Rules and a key: the rules, their settings, and the rule asked for.
	local stored = {
		on_error = "block",
		max_holds = 0,
		rules = { { id = "R1", text = "No rm.", on = { "exec_command.cmd" }, review = 0.3, block = 0.6 } },
	}
	local r = page_run({ constitution = stored }, jev())
	local drawn = said(r:page("rules", { repo = "docbert", rule = "R1" }))
	t.equal(has(drawn, "Rules · 1") and has(drawn, "Nothing checked yet."), true, drawn)
	t.equal(has(drawn, "flags from 0.30, blocks from 0.60"), true)
	t.equal(has(drawn, "Calls it could not check are refused"), true)
	t.equal(has(drawn, "A final answer that breaks a rule stands, flagged."), true)
	t.equal(has(drawn, "Add TypeSafe key"), false)
	t.equal(r:repo_data().nav.detail, "1 rules")
end)
