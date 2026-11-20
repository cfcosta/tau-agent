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

t.case("its view says the step in progress, for the run's header", function()
	local run = t.run({ state = { steps = { { text = "read", status = "done" }, { text = "fix", status = "in_progress" } } } })
	t.equal(run:view().doing, "fix")
	run = t.run({ state = { steps = { { text = "read", status = "done" }, { text = "fix", status = "pending" } } } })
	t.equal(run:view().doing, nil)
end)

t.case("a message keeps a list with steps left", function()
	local run = t.run({ state = { steps = { { text = "read", status = "done" }, { text = "fix", status = "pending" } } } })
	run:start({ input = "go on" })
	t.equal(#run:view().band.children, 2)
end)

-- The band's lines that are steps, and the counts of those cut above
-- and below.
local function band_lines(band)
	local shown, above, below = {}, 0, 0
	for _, line in ipairs(band.children) do
		if line.piece == "toned" then
			local n, word = string.match(line.text, "^%+ (%d+) (%a+)$")
			if word == "done" then
				above = tonumber(n)
			else
				below = tonumber(n)
			end
		else
			table.insert(shown, line)
		end
	end
	return shown, above, below
end

t.case("a long list shows five steps from one before the current, with what is cut", function()
	local list = {}
	for n = 1, 8 do
		table.insert(list, { text = "step " .. n, status = n < 5 and "done" or n == 5 and "in_progress" or "pending" })
	end
	local shown, above, below = band_lines(t.run({ state = { steps = list } }):view().band)
	t.equal({ #shown, above, below }, { 5, 3, 0 })
	list[5].status, list[2].status = "pending", "in_progress"
	list[3].status, list[4].status = "pending", "pending"
	shown, above, below = band_lines(t.run({ state = { steps = list } }):view().band)
	t.equal({ #shown, above, below }, { 5, 0, 3 })
end)

-- Any list of steps: the band shows at most five, the one it is on among
-- them, and what it cuts adds up to the whole list.
t.property("the band shows at most five steps, the current one among them", function(draw)
	local count = draw(g.integers({ min = 1, max = 12 }))
	local on = draw(g.integers({ min = 0, max = count }))
	local list = {}
	for n = 1, count do
		table.insert(list, { text = "step " .. n, status = n < on and "done" or n == on and "in_progress" or "pending" })
	end
	local shown, above, below = band_lines(t.run({ state = { steps = list } }):view().band)
	t.equal(#shown <= 5, true, "at most five")
	t.equal(above + #shown + below, count, "adds up")
	local current = on == 0 and 1 or on
	t.equal(current > above and current <= above + #shown, true, "current shown")
end, { cases = 200 })
