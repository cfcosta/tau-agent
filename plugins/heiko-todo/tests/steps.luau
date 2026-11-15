local heiko = require("heiko")
local t = heiko.test
local g = t.gen

t.case("keeps the steps it is given, replaced whole", function()
	local run = t.run({})
	run:tool("todo_steps", { steps = { { text = "read", status = "done" }, { text = "fix", status = "in_progress" } } })
	run:tool("todo_steps", { steps = { { text = "test", status = "pending" } } })
	t.equal(run.state.steps, { { text = "test", status = "pending" } })
end)

-- What the steps were before the call, kept when it is refused.
local BEFORE = { { text = "earlier", status = "done" } }

-- A right step, in progress often, with no status (pending) now and then.
local right = g.records({
	text = g.sampled_from({ "read", "fix it" }),
	status = g.sampled_from({ "in_progress", "pending", "done", "in_progress" }),
	issue = g.optional(g.just("T-1")),
})
local unstated = g.records({ text = g.just("test") })

-- A step the model might send: mostly a right one, sometimes its text or
-- status wrong, or not a table at all.
local step = g.one_of({
	right,
	right,
	unstated,
	g.records({
		text = g.sampled_from({ "read", "", false, 3 }),
		status = g.optional(g.sampled_from({ "in_progress", "done", "doing", 2 })),
	}),
	g.sampled_from({ "read", 7 }),
})

-- What a list of steps comes to: the steps kept, each with its status
-- (pending when it has none) and its issue, or the words that say why
-- the first thing wrong is.
local function expected(list)
	if type(list) ~= "table" then
		return nil, "steps are a list of { text, status }"
	end
	local kept, going = {}, 0
	for at, given in ipairs(list) do
		if type(given) ~= "table" or type(given.text) ~= "string" or given.text == "" then
			return nil, "step " .. at .. " needs its text"
		end
		local status = given.status or "pending"
		if status ~= "pending" and status ~= "in_progress" and status ~= "done" then
			return nil, "step " .. at .. " has the status `" .. tostring(status) .. "`"
		end
		if status == "in_progress" then
			going += 1
		end
		kept[at] = { text = given.text, status = status, issue = given.issue }
	end
	if going > 1 then
		return nil, "at most one step is in progress; " .. going .. " are"
	end
	return kept
end

-- Any list is taken iff every step has text and a known status and at
-- most one is in progress: then it is the run's steps, normalised; else
-- the error says the first thing wrong and the steps are as they were.
t.property("takes a list of steps iff it is one, and says why not", function(draw)
	local list = draw(g.one_of({ g.lists(step, { max = 6 }), g.sampled_from({ "steps", 1 }) }))
	local want, why = expected(list)
	local run = t.run({ state = { steps = table.clone(BEFORE) } })
	local ok, err = pcall(function()
		run:tool("todo_steps", { steps = list })
	end)
	t.equal(ok, want ~= nil, "taken")
	if ok then
		t.equal(run.state.steps, want, "kept")
	else
		t.equal(string.find(tostring(err), why, 1, true) ~= nil, true, tostring(err) .. " says " .. why)
		t.equal(run.state.steps, BEFORE, "unchanged")
	end
end, { cases = 300 })

t.case("refuses two steps in progress, and says how many are", function()
	local ok, err = pcall(function()
		t.run({}):tool("todo_steps", { steps = { { text = "a", status = "in_progress" }, { text = "b", status = "in_progress" } } })
	end)
	t.equal(ok, false)
	t.equal(string.find(err, "at most one step is in progress; 2 are", 1, true) ~= nil, true, err)
end)

t.case("its band says the step it is on and which of how many", function()
	local run = t.run({ state = { steps = { { text = "read", status = "done" }, { text = "fix", status = "in_progress" } } } })
	local lead = run:view().band.lead
	t.equal({ lead[2].text, lead[3].text }, { "2 of 2", "fix" })
end)
