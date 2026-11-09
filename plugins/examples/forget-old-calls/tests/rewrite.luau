local tau = require("tau")
local t = tau.test

-- A run's messages as `rewrite_context` sees them: the person's, then
-- `calls` answers that each call a tool, and their results.
local function messages(calls)
	local list = { { index = 1, role = "user", text = "fix it" } }
	for n = 1, calls do
		table.insert(list, { index = #list + 1, role = "assistant", text = "", calls = { { id = "c" .. n, name = "read" } } })
		table.insert(list, { index = #list + 1, role = "toolResult", text = "...", call = "c" .. n, name = "read", ok = true })
	end
	return list
end

t.case("leaves a short run alone", function()
	local run = t.run({ settings = { after = 3 } })
	t.equal(run:rewrite_context({ trigger = "turn_end", messages = messages(3) }), nil)
end)

t.case("keeps the person's words and the last messages", function()
	local run = t.run({ settings = { after = 3 } })
	local answer = run:rewrite_context({ trigger = "turn_end", messages = messages(4) })
	t.equal(answer.rewrite.keep, { 1, 4, 5, 6, 7, 8, 9 })
	t.equal(answer.rewrite.drops_conversation, true)
end)

t.case("says what it left out, before what it kept", function()
	local run = t.run({ settings = { after = 3 } })
	local answer = run:rewrite_context({ trigger = "turn_end", messages = messages(4) })
	t.equal(answer.rewrite.insert, {
		{ before = 4, text = "Older tool calls were left out to save room." },
	})
	t.equal(run.state.rewrites, 1)
end)
