-- The demo's run, in the shape the plugin keeps, and what it draws at
-- each step: it starts with a key, cuts the test run's output as it
-- arrives, then prunes the history between turns.
local heiko = require("heiko")
local t = heiko.test

local ARCHIVE = "/home/you/.local/share/heiko/repos/heiko-agent-3f2a91c0/archive/heiko-output-41822-1790716482-0.txt"

local OUTPUTS = {
	{
		call_id = "c7",
		lines = 4810,
		chunks = 241,
		kept = 11,
		dropped_lines = 4598,
		segments = 1,
		requests = 2,
		tokens_before = 14200,
		tokens_after = 1100,
		pruned = true,
		archive = ARCHIVE,
		cost = 0.0006,
	},
}

-- What Jev decided of each judged call: its action, and the odds that
-- the call and its result matter.
local DECIDED = {
	{ "c2", "grep", "keep", 0.64, 0.22 },
	{ "c3", "read", "drop_result", 0.71, 0.18 },
	{ "c5", "edit", "keep", 0.94, 0.88 },
	{ "c6", "exec_command", "keep", 0.18, 0.04 },
	{ "c7", "exec_command", "keep", 0.62, 0.03 },
	{ "c8", "read", "drop_result", 0.52, 0.09 },
	{ "c9", "read", "drop_result", 0.58, 0.11 },
}

-- Every call before the rewrite, as the ledger page lists it, with its
-- short argument: the first ones pinned with the prompt, the rest
-- judged.
local CALLS = {
	{ "c1", "memory_read", "n-0388 · Retry policy honors server hints", 40 },
	{ "c1a", "ls", "crates/heiko-ai/src", 90 },
	{ "c2", "grep", '"retry_after|RetryPolicy" crates/heiko-ai/src', 120 },
	{ "c3", "read", "crates/heiko-ai/src/retry.rs", 3100 },
	{ "c4", "edit", "crates/heiko-ai/src/retry.rs", 210 },
	{ "c5", "edit", "crates/heiko-ai/src/retry.rs", 640 },
	{ "c5t", "exec_command", "cargo nextest run -p heiko-ai", 1900 },
	{ "c6", "exec_command", "rm -rf target/debug/incremental", 20 },
	{ "c7", "exec_command", "cargo nextest run --workspace", 1100 },
	{ "c8", "read", "crates/heiko-ai/src/client.rs", 7600 },
	{ "c9", "read", "crates/heiko-testing/src/fake_openai.rs", 5000 },
}

local function decided_of(call_id)
	for _, one in ipairs(DECIDED) do
		if one[1] == call_id then
			return one
		end
	end
	return nil
end

-- The three states: started, the output cut, the history pruned.
local function demo_states()
	local started = { on = true }
	local cut = { on = true, outputs = OUTPUTS }
	local decisions = array({})
	for _, one in ipairs(DECIDED) do
		table.insert(decisions, { call_id = one[1], tool = one[2], action = one[3], keep_call = one[4], keep_result = one[5] })
	end
	local ledger = array({})
	for _, call in ipairs(CALLS) do
		local judged = decided_of(call[1])
		table.insert(ledger, {
			call_id = call[1],
			tool = call[2],
			input = call[3],
			tokens = call[4],
			matters = judged and judged[4] or json.null,
			verbatim = judged and judged[5] or json.null,
			decision = judged and judged[3] or "pinned",
		})
	end
	local pruned = {
		on = true,
		outputs = OUTPUTS,
		decisions = decisions,
		ledger = ledger,
		last_pass = 172000,
		passes = {
			{
				before = 172000,
				after = 81000,
				results_dropped = 3,
				calls_dropped = 0,
				reduction_ratio = 0.53,
				detail = "7 calls judged in 1 Jev request · 3 results cut, 0 calls dropped · −53% · Jev read 18k tokens of history, whole",
				window = 272000,
			},
		},
	}
	return { started, cut, pruned }
end

t.fixture("run", function()
	local steps = array({})
	for _, state in ipairs(demo_states()) do
		local run = t.run({ id = "demo", settings = {}, jev = {}, state = table.clone(state) })
		table.insert(steps, { state = state, view = run:view() })
	end
	return steps
end)
