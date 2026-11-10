-- The demo's run, what the plugin leaves and draws at each check, and
-- the repository's page as the plugin draws it from the store the run
-- left.
local tau = require("tau")
local t = tau.test

-- What Jev charges per million bytes of a request, as tau's fake Jev
-- counts them: one token a byte.
local PRICE_PER_MILLION_INPUT = 0.042

-- The demo's rules, as its tau-agent repository stores them: tau's own,
-- as the mockups show them.
local function demo_rules()
	local function rule(id, text, on, review, block)
		return { id = id, text = text, on = on, review = review, block = block }
	end
	return {
		on_error = "allow",
		max_holds = 3,
		rules = {
			rule("R1", "Never delete outside target/ or rewrite published history.", { "exec_command.cmd" }, 0.30, 0.60),
			rule(
				"R2",
				"Library code returns errors. No unwrap or expect outside tests.",
				{ "edit.newText", "write.content" },
				0.30,
				0.80
			),
			rule("R3", "Every sqlx query uses the checked macros.", { "edit.newText", "write.content" }, 0.40, 0.85),
			rule("R4", "Comments explain why, not what the code does.", { "edit.newText" }, 0.50, 0.90),
			rule("R5", "No network calls from tests except the fake server.", { "write.content" }, 0.35, 0.80),
			rule("R6", "The final answer names the tests that ran and their result.", { "final answer" }, 0.40, 0.75),
		},
	}
end

-- The request as Jev gets it: each yes/no question as
-- `{ type, instructions, criteria = { true, false } }`.
local function wire(request)
	local questions = {}
	for id, q in pairs(request.questions) do
		questions[id] = {
			type = q.kind,
			instructions = q.question,
			criteria = (q.yes ~= nil or q.no ~= nil) and { ["true"] = q.yes, ["false"] = q.no } or nil,
		}
	end
	return { state = request.state, questions = questions }
end

-- A Jev that scores each rule with the next of its `answers` for it,
-- the last for every later one, and 0 for a rule it has none for: by
-- the rule's id, read from the question. It charges for the bytes of
-- the request it gets.
local function jev(answers)
	local asked = {}
	return {
		ask = function(request)
			local wired = wire(request)
			local ids = {}
			for id in pairs(wired.questions) do
				table.insert(ids, id)
			end
			table.sort(ids)
			local scores = {}
			for _, id in ipairs(ids) do
				local text = json.encode(wired.questions[id])
				local rule, list = "", { 0 }
				for name, given in pairs(answers) do
					if string.find(text, "Rule " .. name .. ":", 1, true) then
						rule, list = name, given
					end
				end
				local n = asked[rule] or 0
				scores[id] = { probability = list[math.min(n, #list - 1) + 1] }
				asked[rule] = n + 1
			end
			return scores, #json.encode(wired) * PRICE_PER_MILLION_INPUT / 1000000
		end,
	}
end

-- A run in the demo's tau-agent repository on `store`, with `fake` or
-- no Jev.
local function run(store, fake)
	return t.run({ id = "demo", repo = "tau-agent", store = store, jev = fake })
end

local function edit(text)
	return { path = "crates/tau-ai/src/retry.rs", edits = { { oldText = "", newText = text } } }
end

-- The demo's run: it starts, an edit breaks R2, an edit passes, a
-- command is doubtful under R1, the answer is sent back for R6, and the
-- next one names its tests. The state and view after each check, and
-- the store it leaves.
local function demo_run()
	local fake = jev({
		R1 = { 0.41 },
		R2 = { 0.95, 0.02 },
		R3 = { 0.04, 0.03 },
		R4 = { 0.08, 0.06 },
		R6 = { 0.91, 0.04 },
	})
	local store = { constitution = demo_rules() }
	local r = run(store, fake)
	-- The view is drawn with no Jev.
	local viewer = run(store, nil)
	r:start({ input = "Make the HTTP client honor retry-after on 429s and 503s" })
	local steps = array({})
	-- Each step as JSON then: the hooks after it change the same state.
	local function step()
		viewer.state = r.state
		table.insert(steps, json.decode(json.encode({ state = r.state, view = viewer:view() })))
	end
	r:before_tool({
		id = "c4",
		name = "edit",
		args = edit("let secs: u64 = header.to_str().unwrap().parse().unwrap();"),
	})
	step()
	r:before_tool({ id = "c5", name = "edit", args = edit("fn server_hint(headers: &HeaderMap) -> Option<Duration> {") })
	step()
	r:before_tool({ id = "c6", name = "exec_command", args = { cmd = "rm -rf target/debug/incremental" } })
	step()
	r:before_stop({ text = "Done. retry-after now overrides the backoff.", turn = 8, reason = "stop" })
	step()
	r:before_stop({ text = "Tests: cargo nextest run -p tau-ai retry::, 14 passed.", turn = 9, reason = "stop" })
	step()
	r:run_end({ stop = "Stop", turns = 9, cost = 0, text = "" })
	return steps, store
end

-- What the page `rules` of tau-agent draws from `store`, a Jev key
-- saved or not.
local function page_of(store, keyed)
	local r = run(store, keyed and jev({ new = { 0.82 } }) or nil)
	return r:page("rules", { repo = "tau-agent" })
end

-- Carries out the page's `action` with `args` on `store`, with a Jev.
local function act(store, action, args)
	run(store, jev({ new = { 0.82 } })):action(action, args)
end

-- The rule the demo's editor writes.
local RULE = "Never run migrations against the production database."

t.fixture("run", function()
	return (demo_run())
end)

-- The page's states the demo shows, by its screens' names.
t.fixture("pages", function()
	local _, store = demo_run()
	local pages = {}
	pages.constitution = page_of(store, true)
	-- A rule being written, on the shell command, and tried on the run's.
	act(store, "open", {})
	act(store, "write", { text = RULE })
	act(store, "place", { place = "exec_command.cmd" })
	act(store, "try", {})
	pages["rule-editor"] = page_of(store, true)
	-- One saved with nowhere to apply.
	act(store, "open", {})
	act(store, "write", { text = RULE })
	act(store, "save", {})
	pages["rule-missing"] = page_of(store, true)
	act(store, "close", {})
	-- Rules that cannot be read: R4 flags past where it blocks.
	local broken = demo_rules()
	broken.rules[4].review = 0.95
	pages["rules-broken"] = page_of({ constitution = broken }, true)
	-- None, once removed, and no key to try them with.
	act(store, "reset", {})
	pages["rules-empty"] = page_of(store, false)
	return pages
end)

-- The demo run checks as its script says: R2 blocks c4, R1 flags c6 and
-- waits for review, R6 holds; the store keeps the one review and the
-- run's two answers.
t.case("the demo run checks as its script says", function()
	local steps, store = demo_run()
	local last = steps[5]
	t.equal(last.state.stats.blocked, { "R2" })
	t.equal(last.state.stats.flagged, { "R1" })
	t.equal(last.state.stats.held, { "R6" })
	t.equal(last.state.stats.calls, 3)
	t.equal(last.state.calls.c4.verdict.kind, "blocked")
	t.equal(last.view.calls.c4.mark, "blocked")
	t.equal(last.view.calls.c6.mark, "flagged")
	t.equal(last.view.status.text, "1 blocked · 1 flagged · 1 held")
	local waiting = {}
	for key, item in pairs(store) do
		if string.sub(key, 1, 7) == "review/" then
			table.insert(waiting, item)
		end
	end
	t.equal(#waiting, 1)
	t.equal(waiting[1].rule, "R1")
	t.equal(store["run/demo"].stats.answers, 2)
end)
