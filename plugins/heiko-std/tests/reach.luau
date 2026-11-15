-- `heiko.store`, `heiko.decide`, `heiko.infer` and `heiko.now`: what a hook
-- awaits, called as a hook calls it, on the harness's fakes.
local heiko = require("heiko")
local t = heiko.test
local g = t.gen

t.case("the store reads what it wrote, once the step ended", function()
	local run = t.run({})
	run:call(function()
		heiko.store.put("a/1", { n = 1 })
		heiko.store.put("a/2", { n = 2 })
		heiko.store.put("b/1", "other")
	end)
	t.equal(run:call(function()
		return heiko.store.get("a/1")
	end), { n = 1 })
	t.equal(run:call(function()
		return heiko.store.get("nothing")
	end), nil)
	local listed = run:call(function()
		return heiko.store.list("a/")
	end)
	t.equal(#listed, 2)
	t.equal({ listed[1].key, listed[2].key }, { "a/1", "a/2" })
	run:call(function()
		heiko.store.delete("a/1")
	end)
	t.equal(run:call(function()
		return heiko.store.get("a/1")
	end), nil)
end)

t.case("bump counts each change, from none", function()
	local run = t.run({})
	for n = 1, 3 do
		run:call(function()
			heiko.store.bump()
		end)
		t.equal(run:call(function()
			return heiko.store.get("rev")
		end), n)
	end
end)

t.property("the store holds the last value put under each key", function(draw)
	local run = t.run({})
	local model = {}
	for _ = 1, draw(g.integers({ min = 1, max = 12 })) do
		local key = draw(g.from_regex("[a-c]"))
		if draw(g.booleans()) then
			local value = draw(g.integers({ min = 0, max = 9 }))
			model[key] = value
			run:call(function()
				heiko.store.put(key, value)
			end)
		else
			model[key] = nil
			run:call(function()
				heiko.store.delete(key)
			end)
		end
		for _, name in ipairs({ "a", "b", "c" }) do
			t.equal(run:call(function()
				return heiko.store.get(name)
			end), model[name], name)
		end
	end
end)

t.case("The decider's answers come with what they cost", function()
	local asked
	local run = t.run({
		decide = {
			ask = function(request)
				asked = request
				return { met = { probability = 0.9 } }, 0.002
			end,
		},
	})
	local request = { state = { x = 1 }, questions = { met = { kind = "predicate", question = "Is it met?" } } }
	t.equal(run:call(function()
		return { heiko.decide.ask(request) }
	end), { { met = { probability = 0.9 } }, 0.002 })
	t.equal(asked.questions.met.kind, "predicate")
end)

t.case("The decider failing is nil and why, not an error", function()
	local run = t.run({
		decide = {
			ask = function()
				error("The decider answered 503", 0)
			end,
		},
	})
	local got = run:call(function()
		local answers, why = heiko.decide.ask({ questions = {} })
		return { answers == nil, why }
	end)
	t.equal(got[1], true)
	t.equal(string.find(got[2], "503", 1, true) ~= nil, true, got[2])
end)

t.case("scores are each question's probability, in the order asked", function()
	local run = t.run({
		decide = {
			ask = function()
				return { a = { probability = 0.25 }, b = { probability = 0.75 } }, 0.001
			end,
		},
	})
	t.equal(run:call(function()
		return { heiko.decide.scores({ questions = {} }, { "b", "a" }) }
	end), { { 0.75, 0.25 }, 0.001 })
	-- One it left out, or answered without a probability, is no score.
	local missing = run:call(function()
		local scores, why = heiko.decide.scores({ questions = {} }, { "a", "c" })
		return { scores == nil, why }
	end)
	t.equal(missing, { true, "The decider gave no answer to c" })
end)

t.case("the model's answer is its envelope, and a refusal says why", function()
	local answers = {
		{ ok = true, value = "a summary" },
		{ ok = false, error = "refused", reason = "policy" },
	}
	local asked = 0
	local run = t.run({
		tools = {
			infer = function()
				asked += 1
				return answers[asked]
			end,
		},
	})
	t.equal(run:call(function()
		return heiko.infer({ task = "summarize" })
	end), { ok = true, value = "a summary" })
	local refused = run:call(function()
		return heiko.infer({ task = "summarize" })
	end)
	t.equal(refused.ok, false)
	t.equal(refused.error, "refused")
	t.equal(refused.reason, "policy")
end)

t.case("now is the step's time, which a test sets", function()
	local run = t.run({})
	run:at("2026-11-15T10:00:00Z")
	local first = run:call(function(_, env)
		return heiko.now(env)
	end)
	run:at("2026-11-15T10:00:30Z")
	local second = run:call(function(_, env)
		return heiko.now(env)
	end)
	t.equal(second - first, 30)
end)
