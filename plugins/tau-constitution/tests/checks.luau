-- The checks in a run: a call a rule forbids is refused with the rule,
-- a doubtful one runs and is flagged, a final answer that breaks a rule
-- goes back up to the cap, `on_error` decides wherever Jev fails, an
-- edit applies from the next call, and rules that cannot be read fail
-- the run; and what the view and the store make of every check.
local tau = require("tau")
local t = tau.test
local g = t.gen
local rules = require("rules")
local checks = require("checks")

local R2 = "Library code returns errors. No unwrap or expect."
local R6 = "The final answer names the tests that ran."

-- R2 on what `write` writes, R6 on the final answer.
local function constitution(on_error, max_holds)
	return {
		on_error = on_error or "allow",
		max_holds = max_holds or 3,
		rules = {
			{ id = "R2", text = R2, on = { "write.content" }, review = 0.3, block = 0.8 },
			{ id = "R6", text = R6, on = { "final answer" }, review = 0.4, block = 0.75 },
		},
	}
end

-- A Jev scoring every question `score(request)`, keeping each request
-- in `asked`; a score that is an error string fails the request.
local function jev(score)
	local asked = {}
	return {
		ask = function(request)
			table.insert(asked, request)
			local p = score(request, #asked)
			if type(p) == "string" then
				error(p, 0)
			end
			local answers = {}
			for id in pairs(request.questions) do
				answers[id] = { probability = p }
			end
			return answers, 0.001
		end,
	}, asked
end

local function run(fake, stored)
	local r = t.run({ jev = fake, store = { constitution = stored or constitution() } })
	r:start({ input = "fix it" })
	return r
end

local function write(id, content)
	return { id = id, name = "write", args = { path = "src/lib.rs", content = content } }
end

local function stop(text)
	return { text = text, turn = 1, reason = "stop" }
end

local function has(text, part)
	return string.find(text, part, 1, true) ~= nil
end

t.case("a call that breaks a rule is refused with the rule", function()
	local fake, asked = jev(function(request)
		return string.find(json.encode(request.state), "unwrap", 1, true) and 0.95 or 0.05
	end)
	local r = run(fake)
	local decision = r:before_tool(write("c1", "let x = y.unwrap();"))
	t.equal(decision.decision, "block")
	t.equal(has(decision.reason, "Blocked by tau-constitution"), true, decision.reason)
	t.equal(has(decision.reason, "rule R2"), true)
	t.equal(has(decision.reason, "No unwrap or expect"), true)
	-- Jev saw the tool and only the field the rule names.
	t.equal(asked[1].state, { tool = "write", arguments = { content = "let x = y.unwrap();" } })
	local questions = 0
	for _ in pairs(asked[1].questions) do
		questions += 1
	end
	t.equal(questions, 1)
	t.equal(has(asked[1].questions["rule-0"].question, "Rule R2: " .. R2), true)
	-- The call's check with every score, its verdict, then the check of
	-- the final answer, which passed.
	t.equal(r:before_stop(stop("I ran cargo test: 3 passed.")), nil)
	local stats = r.state.stats
	t.equal({ stats.calls, stats.answers, stats.questions, #stats.blocked }, { 1, 1, 2, 1 })
	t.equal(stats.blocked[1], "R2")
	t.equal(stats.cost > 0, true, "Jev's cost is counted")
	t.equal(r.state.calls.c1.scores, { { rule = "R2", score = 0.95 } })
	t.equal(r.state.calls.c1.verdict.kind, "blocked")
	-- The card shows it, and the store keeps what the page shows.
	local view = r:view()
	t.equal(view.calls.c1.mark, "blocked")
	t.equal(view.calls.c1.reason, decision.reason)
	t.equal(view.calls.c1.body ~= nil, true)
	t.equal(view.status, tau.ui.status("1 blocked", nil, "bad"))
	t.equal(r.store.handled[1].what, "blocked")
	t.equal(r.store["run/test"].stats.calls, 1)
end)

-- A score at or near a rule's thresholds, or anywhere from 0 to 1.
local function near(review, block)
	local step = 1e-9
	return g.one_of({
		g.sampled_from({ review - step, review, block - step, block }),
		g.floats({ min = 0, max = 1 }),
	})
end

t.property("verdicts follow the rules' thresholds", function(draw)
	-- R2 on the call (0.3, 0.8), R6 on the answer (0.4, 0.75).
	local on_call = draw(near(0.3, 0.8))
	local on_answer = draw(near(0.4, 0.75))
	local answered = false
	local fake = jev(function(request)
		if request.state.tool then
			return on_call
		elseif not answered then
			answered = true
			return on_answer
		end
		return 0
	end)
	local r = run(fake)
	local decision = r:before_tool(write("c1", "maybe"))
	local first = r:before_stop(stop("Tests: cargo test, all passed."))
	local held = on_answer >= 0.75
	t.equal(first ~= nil, held)
	if held then
		t.equal(r:before_stop(stop("Tests: cargo test, 3 passed.")), nil)
	end
	local blocked = on_call >= 0.8
	t.equal(decision ~= nil and decision.decision == "block", blocked)
	local expected = {}
	if blocked then
		table.insert(expected, "blocked R2")
	elseif on_call >= 0.3 then
		table.insert(expected, "flagged R2")
	end
	if held then
		table.insert(expected, "held R6")
	elseif on_answer >= 0.4 then
		table.insert(expected, "flagged R6")
	end
	local got = {}
	local verdict = r.state.calls.c1.verdict
	if verdict then
		table.insert(got, verdict.kind .. " " .. verdict.rule)
	end
	for _, rule in ipairs(r.state.stats.held) do
		table.insert(got, "held " .. rule)
	end
	for _, flag in ipairs(r.state.stats.flagged_answers) do
		table.insert(got, "flagged " .. flag.rule)
	end
	t.equal(got, expected)
end, { cases = 200 })

t.case("a final answer that breaks a rule goes back until the cap", function()
	-- The first answer breaks R6; the second follows it.
	local fake = jev(function(_, n)
		return n == 1 and 0.9 or 0.1
	end)
	local r = run(fake)
	local held = r:before_stop(stop("Done."))
	t.equal(held.decision, "continue")
	t.equal(has(held.message, "rule R6"), true, held.message)
	t.equal(r:before_stop(stop("Done. cargo test: 12 passed.")), nil)
	t.equal(r.state.notes.c0.detail, "before_stop · continuation 1 / 3")
	t.equal(r.state.stats.held, { "R6" })
	-- The message it sent back draws as nothing: its note says it.
	t.equal(r:view().messages[held.message], tau.ui.stack({}))

	-- Past the cap, a broken answer stands, flagged.
	local capped = run(jev(function()
		return 0.9
	end), constitution("allow", 1))
	t.equal(capped:before_stop(stop("Done.")).decision, "continue")
	t.equal(capped:before_stop(stop("Still done.")), nil)
	t.equal(capped.state.stats.held, { "R6" })
	t.equal(capped.state.stats.flagged, { "R6" })
	t.equal(capped.state.stats.flagged_answers[1].answer, "Still done.")
	-- The next message may send it back again.
	capped:start({ input = "go on" })
	t.equal(capped:before_stop(stop("Done again.")).decision, "continue")
end)

t.property("on_error decides wherever Jev fails", function(draw)
	local block = draw(g.booleans())
	local max_holds = draw(g.integers({ min = 0, max = 3 }))
	-- Whether each of Jev's requests fails; past the list, they pass.
	local fails = draw(g.lists(g.booleans(), { max = 8 }))
	local writes = draw(g.integers({ min = 0, max = 2 }))
	local failed = 0
	local fake = jev(function(_, n)
		if fails[n] then
			failed += 1
			return "offline"
		end
		-- Passing: every rule kept.
		return 0
	end)
	local r = run(fake, constitution(block and "block" or "allow", max_holds))
	local refused = 0
	for n = 1, writes do
		local decision = r:before_tool(write("c" .. n, "x"))
		if decision then
			t.equal(has(decision.reason, "cannot check"), true, decision.reason)
			refused += 1
		end
	end
	-- Answers, until one stands.
	local continued = 0
	for _ = 0, max_holds do
		if r:before_stop(stop("Done.")) == nil then
			break
		end
		continued += 1
	end
	-- Each failure gets a note saying what `on_error` did.
	t.equal(r.state.stats.failed, failed)
	local notes, failed_calls, holds = 0, 0, {}
	for key, note in pairs(r.state.notes) do
		notes += 1
		if has(note.detail, "blocked") or has(note.detail, "ran") then
			failed_calls += 1
		end
		if has(note.detail, "sent back") or has(note.detail, "stands") then
			holds[tonumber(string.sub(key, 2)) + 1] = has(note.detail, "sent back")
		end
	end
	t.equal(notes, failed)
	t.equal(refused, block and failed_calls or 0)
	t.equal(continued <= max_holds, true)
	if not block then
		t.equal(continued, 0, "allow lets every answer stand")
	else
		-- Each failure holds until the holds run out.
		local sent = 0
		for n = 1, notes do
			if holds[n] ~= nil then
				t.equal(holds[n], sent < max_holds)
				if holds[n] then
					sent += 1
				end
			end
		end
		t.equal(sent, continued)
	end
end, { cases = 200 })

t.case("checks Jev could not answer say what on_error did", function()
	local state = checks.state({})
	for _, r in ipairs({
		{ kind = "error", call_id = "c1", tool = "write", message = "Jev could not check the call: offline", on_error = "block", held = false },
		{ kind = "error", call_id = "c2", tool = "write", message = "Jev could not check the call: offline", on_error = "allow", held = false },
		{ kind = "error", message = "Jev could not check the final answer: offline", on_error = "block", held = true },
		{ kind = "error", message = "Jev could not check the final answer: offline", on_error = "allow", held = false },
	}) do
		checks.fold(state, r)
	end
	local details = {}
	for n = 0, 3 do
		table.insert(details, state.notes["c" .. n].detail)
	end
	t.equal(details, { "not checked · blocked", "not checked · ran", "not checked · sent back", "not checked · the answer stands" })
	t.equal(state.stats.failed, 4)
	t.equal(state.notes.c0.tone, "bad")
end)

t.case("an edit applies from the next tool call", function()
	local fake = jev(function()
		return 0.9
	end)
	local r = run(fake, rules.default())
	-- No rules at the first call: it runs unchecked.
	t.equal(r:before_tool(write("c1", "x.unwrap()")), nil)
	-- Rules saved meanwhile check the second, in the same run.
	r.store.constitution = constitution()
	local decision = r:before_tool(write("c2", "y.unwrap()"))
	t.equal(decision.decision, "block")
	t.equal(r.state.calls.c2.verdict.rule, "R2")
	t.equal(r.state.calls.c1, nil)
end)

t.case("a run says what it watches, and rules that cannot be read fail it", function()
	local fake = jev(function()
		return 0
	end)
	local function starting(spec)
		local r = t.run(spec)
		local answer = r:start({ input = "fix it" })
		return answer, r.state.starting
	end
	t.equal({ starting({ store = { constitution = constitution() } }) }, { nil, checks.NO_KEY })
	t.equal({ starting({ jev = fake, store = { constitution = constitution() } }) }, { nil, "watching 2 rules" })
	t.equal({ starting({ jev = fake, store = {} }) }, { nil, "no rules" })
	local one = constitution()
	table.remove(one.rules)
	t.equal({ starting({ jev = fake, store = { constitution = one } }) }, { nil, "watching 1 rule" })
	-- Flagged past where it blocks: it does not check.
	local broken = constitution()
	broken.rules[1].review = 0.95
	local failed, said = starting({ jev = fake, store = { constitution = broken } })
	t.equal(said, "the rules cannot be read")
	t.equal(failed, tau.fail(rules.errors.invalid("test", "Rule R2: review (0.95) is above block (0.8)")))
	-- Without a key nothing is checked, so nothing fails yet.
	t.equal({ starting({ store = { constitution = broken } }) }, { nil, checks.NO_KEY })
	-- Nor is a call checked without one.
	local keyless = t.run({ store = { constitution = constitution() } })
	keyless:start({ input = "fix it" })
	t.equal(keyless:before_tool(write("c1", "x.unwrap()")), nil)
	t.equal(keyless:view().status, tau.ui.status(checks.NO_KEY, nil, "neutral"))
end)

t.property("a constitution comes back from the store as saved", function(draw)
	local saved = rules.default()
	saved.on_error = draw(g.sampled_from({ "allow", "block" }))
	saved.max_holds = draw(g.integers({ min = 0, max = 9 }))
	for _ = 1, draw(g.integers({ min = 0, max = 5 })) do
		local review = draw(g.floats({ min = 0, max = 1 }))
		local block = draw(g.floats({ min = review, max = 1 }))
		local on = draw(g.lists(g.sampled_from({ "write.content", "exec_command.cmd", "final answer" }), { min = 1, max = 3 }))
		t.equal(rules.add(saved, "rule " .. draw(g.text({ max = 20 })), on, review, block) ~= nil, true)
		-- Removing one now and then leaves gaps in the ids.
		if draw(g.booleans()) then
			rules.remove(saved, saved.rules[1].id)
		end
	end
	local back = rules.from_stored(json.decode(json.encode(saved)))
	t.equal(back, saved)
	local r = t.run({ jev = jev(function()
		return 0
	end), store = { constitution = json.decode(json.encode(saved)) } })
	r:start({ input = "go" })
	t.equal(r.state.starting, checks.starting(true, #saved.rules))
end, { cases = 100 })

-- One of the plugin's records: a check of a call or the answer, a
-- verdict on either, or a failure.
local record = g.composite(function(draw)
	local rule = draw(g.sampled_from({ "R1", "R2", "R3" }))
	local call = draw(g.optional(g.sampled_from({ "c1", "c2" })))
	local score = draw(g.integers({ min = 0, max = 100 })) / 100
	local kind = draw(g.integers({ min = 0, max = 4 }))
	if kind == 0 then
		local scores = array({})
		for n = 1, draw(g.integers({ min = 0, max = 3 })) do
			table.insert(scores, { rule = "R" .. n, score = score })
		end
		-- Multiples of 1/1024, so sums are exact.
		return { kind = "checked", call_id = call, scores = scores, cost = draw(g.integers({ min = 0, max = 64 })) / 1024 }
	elseif kind == 1 then
		return {
			kind = "blocked",
			rule = rule,
			text = "text of " .. rule,
			score = score,
			call_id = call,
			reason = draw(g.optional(g.just("Rule says no"))),
		}
	elseif kind == 2 then
		return { kind = "flagged", rule = rule, text = "text of " .. rule, score = score, call_id = call }
	elseif kind == 3 then
		return {
			kind = "held",
			rule = rule,
			text = "text of " .. rule,
			score = score,
			hold = 1,
			max_holds = draw(g.integers({ min = 1, max = 5 })),
		}
	end
	return {
		kind = "error",
		message = "offline",
		call_id = call,
		on_error = draw(g.sampled_from({ "allow", "block" })),
		held = draw(g.booleans()),
	}
end)

t.property("the fold marks what the checks decided", function(draw)
	local records = draw(g.lists(record, { max = 10 }))
	local state = checks.state({})
	for _, r in ipairs(records) do
		checks.fold(state, r, "Done.", "shown")
	end
	local function rules_of(kind)
		local list = {}
		for _, r in ipairs(records) do
			if r.kind == kind then
				table.insert(list, r.rule)
			end
		end
		return list
	end
	local expected = checks.stats()
	local max_holds
	for _, r in ipairs(records) do
		if r.kind == "checked" then
			if r.call_id then
				expected.calls += 1
			else
				expected.answers += 1
			end
			expected.questions += #r.scores
			expected.cost += r.cost
		elseif r.kind == "error" then
			expected.failed += 1
		elseif r.kind == "held" then
			max_holds = r.max_holds
		elseif r.kind == "flagged" and r.call_id == nil then
			table.insert(expected.flagged_answers, { rule = r.rule, text = r.text, score = r.score, answer = "Done." })
		end
	end
	expected.blocked, expected.flagged, expected.held = rules_of("blocked"), rules_of("flagged"), rules_of("held")
	expected.max_holds = max_holds
	t.equal(state.stats, expected)
	local view = tau.test.load().view(state, {})
	for _, id in ipairs({ "c1", "c2" }) do
		local verdict, scores
		local grave = { flagged = 1, blocked = 2 }
		for _, r in ipairs(records) do
			if r.call_id == id and (r.kind == "blocked" or r.kind == "flagged") then
				-- The gravest verdict stands, the first of those.
				if verdict == nil or grave[r.kind] > grave[verdict.kind] then
					verdict = r
				end
			elseif r.call_id == id and r.kind == "checked" then
				scores = r.scores
			end
		end
		local call = state.calls[id]
		if verdict == nil and scores == nil then
			t.equal(call, nil, id)
		else
			t.equal(call.scores, scores or {}, id)
			t.equal(call.verdict and { call.verdict.rule, call.verdict.score }, verdict and { verdict.rule, verdict.score }, id)
			-- Its card is marked as its gravest verdict says.
			local drawn = view.calls[id] or {}
			t.equal(drawn.mark, verdict and verdict.kind, id)
			if verdict and verdict.kind == "blocked" then
				t.equal(drawn.reason, verdict.reason or verdict.text)
			end
		end
	end
	-- A note for each failure and each verdict on the answer.
	local noted = 0
	for _, r in ipairs(records) do
		if r.kind == "error" or (r.kind ~= "checked" and r.call_id == nil) then
			noted += 1
		end
	end
	local notes = 0
	for _ in pairs(view.notes) do
		notes += 1
	end
	t.equal(notes, noted)
end, { cases = 200 })

t.case("a verdict marks its call, and every call shows its scores", function()
	local state = checks.state({ repo = "tau-agent" })
	for _, r in ipairs({
		{ kind = "blocked", rule = "R1", text = "No deletes.", score = 0.95, call_id = "c1", tool = "exec_command", reason = "Rule R1 forbids it" },
		{ kind = "flagged", rule = "R2", text = "No unwrap.", score = 0.55, call_id = "c2", tool = "write" },
		{ kind = "checked", call_id = "c2", tool = "write", scores = { { rule = "R2", score = 0.55 } }, cost = 0 },
		{ kind = "checked", call_id = "c3", tool = "write", scores = { { rule = "R2", score = 0.05 }, { rule = "R5", score = 0.1 } }, cost = 0 },
	}) do
		checks.fold(state, r, nil, "a.rs")
	end
	local view = tau.test.load().view(state, {})
	t.equal(view.calls.c1.mark, "blocked")
	t.equal(view.calls.c1.reason, "Rule R1 forbids it")
	t.equal(view.calls.c2.mark, "flagged")
	t.equal(state.calls.c2.scores, { { rule = "R2", score = 0.55 } })
	-- A flagged call's badge names the rule, its score and a way to review
	-- it on the repository's page.
	local badge = json.encode(view.calls.c2.badge)
	t.equal(has(badge, "R2 0.55") and has(badge, "Review") and has(badge, "tau-agent"), true, badge)
	-- A passed one shows each score, quietly.
	t.equal(view.calls.c3.badge, tau.ui.mono("R2 0.05 · R5 0.10"))
	t.equal(view.calls.c3.mark, nil)
	t.equal(view.status, tau.ui.status("1 blocked · 1 flagged", nil, "bad"))
end)

t.property("a repository's totals are its runs' counts together", function(draw)
	local runs = {}
	local all = checks.stats()
	for _ = 1, draw(g.integers({ min = 0, max = 5 })) do
		local stats = checks.stats()
		for _, r in ipairs(draw(g.lists(record, { max = 6 }))) do
			checks.count(stats, r, nil)
			checks.count(all, r, nil)
		end
		table.insert(runs, stats)
	end
	local totals = checks.totals(runs)
	t.equal(totals.runs, #runs)
	t.equal(totals.checked, all.calls + all.answers)
	t.equal({ totals.blocked, totals.flagged, totals.failed }, { #all.blocked, #all.flagged, all.failed })
	t.equal(totals.cost, all.cost)
	-- The order runs come in does not matter.
	local reversed = {}
	for n = #runs, 1, -1 do
		table.insert(reversed, runs[n])
	end
	t.equal(checks.totals(reversed), totals)
end, { cases = 200 })

-- Two rules read the same call: one blocks it, the next only flags it.
-- The call was refused, so its card says blocked, by the rule that
-- blocked it, whichever verdict came last.
t.case("a blocked call stays blocked when a later rule only flags it", function()
	for _, order in ipairs({ { "blocked", "flagged" }, { "flagged", "blocked" } }) do
		local state = checks.state({})
		for n, kind in ipairs(order) do
			checks.fold(state, {
				kind = kind,
				call_id = "c1",
				tool = "bash",
				rule = kind == "blocked" and "no-rm" or "careful",
				score = 0.9 - n / 10,
				reason = kind,
			}, nil, "rm -rf /")
		end
		t.equal(state.calls.c1.verdict.kind, "blocked", table.concat(order, " then "))
		t.equal(state.calls.c1.verdict.rule, "no-rm")
	end
end)
