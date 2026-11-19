-- heiko-suggestions in a run, its tick and a repository's page: a run
-- that ends keeps a request and asks no model; the tick asks it and the
-- composer says the newest; a run that starts with its prompt took it;
-- the page dismisses one and asks again; a sub-agent, a cancelled run
-- and the switch off keep nothing; and a refused request keeps the list.
local heiko = require("heiko")
local t = heiko.test

local TRANSCRIPT = array({
	{ role = "user", text = "why does the sync test fail?" },
	{ role = "assistant", text = "", calls = { { name = "exec_command" } } },
	{ role = "toolResult", name = "exec_command", text = "test sync_test ... FAILED" },
	{ role = "assistant", text = "It races on the clock: I pinned it." },
})

local ANSWER = {
	suggestions = {
		{ title = "Add a regression test for the race", prompt = "Add a test that fails when sync_test races on the clock.", why = "the run pinned the clock without a test" },
		{ title = "Check the other clock users", prompt = "Find other tests that read the wall clock.", why = "the same race may be elsewhere" },
	},
}

-- A run in the repository "test" (the harness's), whose model is asked
-- through `infer`, counted in `asked`.
local function run(spec)
	spec = spec or {}
	local asked = { n = 0 }
	spec.transcript = spec.transcript or TRANSCRIPT
	spec.models = { { id = "gpt-6-luna", family = "luna" } }
	spec.tools = spec.tools or {
		infer = function(args)
			asked.n += 1
			asked.last = args
			return { ok = true, value = ANSWER }
		end,
	}
	return t.run(spec), asked
end

t.case("a run that ends keeps a request and asks no model", function()
	local r, asked = run()
	r:run_end({ stop = "Stop", text = "done" })
	t.equal(asked.n, 0)
	t.equal(r.store.request.digest.asks, { "why does the sync test fail?" })
	t.equal(r:repo_data().composer, nil, "the composer keeps its words meanwhile")
end)

t.case("the tick asks, and the composer says the newest", function()
	local r, asked = run()
	r:run_end({ stop = "Stop", text = "done" })
	r:job("suggest")
	t.equal(asked.n, 1)
	t.equal(asked.last.effort, "low")
	t.equal(r.store.request, nil, "asked once")
	t.equal(r.job_state.answered, 1, "its state changed, so the interface draws again")
	local data = r:repo_data()
	t.equal(data.composer, {
		placeholder = "Check the other clock users",
		prompt = "Find other tests that read the wall clock.",
	})
	t.equal(data.nav, { detail = "2" })
	t.equal(r.store.suggestions[1].model, "gpt-6-luna")
	-- Nothing more to ask: the next tick asks nothing.
	r:job("suggest")
	t.equal(asked.n, 1)
end)

t.case("a run that starts with a suggestion's prompt took it", function()
	local r = run()
	r:run_end({ stop = "Stop", text = "done" })
	r:job("suggest")
	r:start({ input = "Find other tests that read the wall clock." })
	t.equal(r:repo_data().composer.placeholder, "Add a regression test for the race")
	-- Taken, it is turned down for the next request.
	r:run_end({ stop = "Stop", text = "done" })
	r:job("suggest")
	t.equal(r.store.suggestions[1].status, "taken")
end)

t.case("the page dismisses one, and asks again from the last run", function()
	local r, asked = run()
	r:run_end({ stop = "Stop", text = "done" })
	r:job("suggest")
	local page = json.encode(r:page("suggested", {}))
	t.equal(string.find(page, "Check the other clock users", 1, true) ~= nil, true)
	t.equal(string.find(page, "heiko.composer", 1, true) ~= nil, true)
	local newest = r.store.suggestions[2].id
	r:action("dismiss", { id = newest })
	t.equal(r:repo_data().composer.placeholder, "Add a regression test for the race")
	t.equal(pcall(r.action, r, "dismiss", { id = newest }), false, "dismissed already")
	r:action("again", {})
	r:job("suggest")
	t.equal(asked.n, 2)
	t.equal(string.find(asked.last.task, "- Check the other clock users", 1, true) ~= nil, true, "turned down")
end)

t.case("asking again before any run ended says why", function()
	local r = run()
	local ok, why = pcall(r.action, r, "again", {})
	t.equal(ok, false)
	t.equal(why, "No run has ended here since suggestions were turned on")
end)

t.case("a sub-agent, a cancelled run and the switch off keep nothing", function()
	local sub = run({ kind = "sub_agent" })
	sub:run_end({ stop = "Stop", text = "done" })
	t.equal(sub.store.request, nil)
	local cancelled = run()
	cancelled:run_end({ stop = "Cancelled", text = "" })
	t.equal(cancelled.store.request, nil)
	local off = run({ settings = { on = false, count = 3 } })
	off:run_end({ stop = "Stop", text = "done" })
	t.equal(off.store.request, nil)
	t.equal(off:repo_data(), {})
end)

t.case("a refused request keeps the list, and is not asked again", function()
	local r = run({
		tools = {
			infer = function()
				return { ok = false, error = "the plan has no luna" }
			end,
		},
		store = { suggestions = { { id = "1-1", title = "Older", prompt = "p", why = "", run = "r0", model = "m", at = 1767603000, status = "pending" } } },
	})
	r:run_end({ stop = "Stop", text = "done" })
	r:job("suggest")
	t.equal(r.store.request, nil)
	t.equal(r:repo_data().composer.placeholder, "Older")
	t.equal(r.job_state.answered, nil)
end)

-- The demo's heiko-agent: a run on the retry policy ended, and the model
-- answered with what may come next.
local function demo_run()
	local r = t.run({
		now = "2026-01-05T09:00:00Z",
		models = { { id = "gpt-6-luna", family = "luna" } },
		transcript = array({
			{ role = "user", text = "Make the retry wait the server's Retry-After header." },
			{ role = "assistant", text = "", calls = { { name = "exec_command" } } },
			{ role = "toolResult", name = "exec_command", text = "test retry::honours_retry_after ... ok" },
			{ role = "assistant", text = "The retry waits Retry-After now, capped at max_delay." },
		}),
		tools = {
			infer = function()
				return {
					ok = true,
					value = {
						suggestions = {
							{
								title = "Log malformed Retry-After headers once",
								prompt = "Log a malformed Retry-After header once per run instead of on every retry.",
								why = "the run parses the header and drops what it cannot read",
							},
							{
								title = "Add jitter on top of the Retry-After hint",
								prompt = "Add jitter on top of the Retry-After hint so clients do not retry in lockstep.",
								why = "every client now waits exactly the server's hint",
							},
						},
					},
				}
			end,
		},
	})
	r:run_end({ stop = "Stop", text = "done" })
	r:job("suggest")
	return r
end

t.case("the demo's composer says the newest suggestion", function()
	t.equal(demo_run():repo_data().composer.placeholder, "Add jitter on top of the Retry-After hint")
end)

t.fixture("demo", function()
	return { repo_data = { ["heiko-agent"] = demo_run():repo_data() } }
end)
