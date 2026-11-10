-- The questions the demo shows waiting, as the plugin leaves and draws
-- them: untouched, a checklist's note being written, and on the review;
-- and another chat waiting on one question.
local tau = require("tau")
local t = tau.test

-- The questions the demo's run waits on.
local function demo_questions()
	return {
		questions = {
			{
				question = "How should the ask tool wait for your answer?",
				header = "Waiting",
				options = {
					{
						label = "Hold the call open (Recommended)",
						description = "The tool blocks on a channel; your answer comes back as its result and the turn goes on.",
						preview = "let (tx, rx) = oneshot::channel();\nself.pending.insert(ctx.call_id, tx);\nselect! {\n    a = rx => Ok(a?),\n    _ = ctx.cancel => Err(Cancelled),\n}",
					},
					{
						label = "End the turn",
						description = "Like vcs_land: the run stops, and your answer arrives as the next message.",
						preview = 'ctx.publish(&asked)?;\nOk(ToolOutput::text("Asked; waiting."))',
					},
					{
						label = "Hold, then time out",
						description = "Block for a while; if nobody answers, stop the run and keep the panel open.",
						preview = "select! {\n    a = rx => Ok(a?),\n    _ = sleep(timeout) => stop(),\n}",
					},
				},
			},
			{
				question = "Where else should a pending question show up?",
				header = "Surfaces",
				multi_select = true,
				options = {
					{ label = "Run list badge", description = "An amber dot on the run in the sidebar until you answer." },
					{ label = "Desktop notification", description = "Only when the window is not focused." },
					{ label = "Parent run", description = "A child run's question surfaces on the run that spawned it." },
				},
			},
			{
				question = "How long should a question hold the run?",
				header = "Timeout",
				options = {
					{ label = "5 minutes", description = "Short; the run stops soon and resumes when you answer." },
					{ label = "Never", description = "Hold until you answer or cancel the run." },
				},
			},
		},
	}
end

-- What run `id`'s call `call_id` asking `asked` leaves after `steps`,
-- each an action and its arguments, and the view drawn from it.
local function demo_of(id, call_id, asked, steps)
	local r = t.run({ id = id, live = true })
	r:tool("ask", asked, call_id)
	for _, step in ipairs(steps) do
		r:action(step[1], step[2])
	end
	return { state = r.state, view = r:view() }
end

local function key(k)
	return { "key", { key = k } }
end

local NOTE = { "note-done", { text = "Only when the run has waited a while." } }
local RUN = "retry-after"

t.fixture("ask", function()
	return demo_of(RUN, "call_ask", demo_questions(), {})
end)

t.fixture("note", function()
	return demo_of(RUN, "call_ask", demo_questions(), { key("1"), key("1"), key("3"), key("n"), NOTE, key("n") })
end)

t.fixture("review", function()
	return demo_of(RUN, "call_ask", demo_questions(), { key("1"), key("1"), key("3"), key("n"), NOTE, key("right"), key("2") })
end)

t.fixture("sweep", function()
	return demo_of("sweep-workspaces", "call_sweep", {
		questions = {
			{
				question = "Delete the 3 workspaces with no run, or keep them?",
				header = "Workspaces",
				options = {
					{ label = "Delete them", description = "No run uses them." },
					{ label = "Keep them", description = "Look at them first." },
				},
			},
		},
	}, {})
end)
