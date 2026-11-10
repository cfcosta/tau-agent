-- tau-fast-compaction's output pruning in a real agent loop (ADR 0053):
-- a fake `exec_command` whose output each loop chooses, a scripted model,
-- and a fake Jev that keeps the chunks holding an error.
local tau = require("tau")
local t = tau.test

local HEADER = "[fast-compaction pruned this output: kept lines are verbatim, omitted lines are marked]"

local function contains(text, part)
	return string.find(text, part, 1, true) ~= nil
end

local function starts_with(text, part)
	return string.sub(text, 1, #part) == part
end

-- A build log of `lines` lines, with an error halfway.
local function build_log(lines)
	local out = {}
	for n = 0, lines - 1 do
		if n == lines // 2 then
			table.insert(out, "error[E0425]: cannot find value `widget` in this scope")
		else
			table.insert(out, string.format("   Compiling crate-number-%d v0.1.0 (/src/crate-%d)", n, n))
		end
	end
	return table.concat(out, "\n")
end

-- Keeps a chunk when its text has an error, and drops the rest.
local KEEPS_ERRORS = { { has = "error[", noul = 0.95 }, { noul = 0.02 } }

-- One `cargo build`, answered by `exec`, then an answer.
local function one_build(exec, more)
	local out = {
		prompt = "fix the build",
		tools = { exec_command = exec },
		jev = KEEPS_ERRORS,
		model = {
			{ tool = "exec_command", args = { cmd = "cargo build" } },
			{ text = "done" },
		},
	}
	for key, value in pairs(more or {}) do
		out[key] = value
	end
	return out
end

-- The text of the result the model's second request ends with.
local function seen_result(run)
	local messages = run.requests[2].messages
	local last = messages[#messages]
	t.equal(last.role, "toolResult", "a tool result")
	t.equal(#last.content, 1, "one block")
	t.equal(last.content[1].type, "text", "a text block")
	return last.content[1].text
end

-- The archive a pruned output names in its footer.
local function footer_archive(text)
	return string.match(text, ".*%[full output: (.-) %(read or grep it if needed%)%]$")
end

-- What the plugin's state says of each output it asked Jev about.
local function outputs(run)
	return type(run.state) == "table" and run.state.outputs or {}
end

local function archived(run)
	local paths = {}
	for path in pairs(run.archive) do
		table.insert(paths, path)
	end
	return paths
end

-- A large output reaches the model pruned: the header, the error line
-- from its middle, markers for what went, and a footer naming an
-- archive, readable only by its owner, that holds the output whole. Jev
-- saw the command, the task and the chunks; its usage is charged, and
-- the plugin's state says how it went, which the call's card shows.
local LOG = build_log(3000)
t.loop("a large output reaches the model pruned", one_build(LOG), function(run)
	t.equal(run.outcome.text, "done")
	local seen = seen_result(run)
	t.equal(starts_with(seen, HEADER), true, seen)
	t.equal(contains(seen, "error[E0425]: cannot find value `widget`"), true, seen)
	t.equal(contains(seen, " lines omitted]"), true, seen)
	t.equal(#seen < #LOG / 2, true, #seen .. " of " .. #LOG)
	local archive = footer_archive(seen)
	t.equal(archive ~= nil, true, seen)
	-- The plugin archives under the loop's own folder.
	t.equal(run.archive[archive] ~= nil, true, archive)
	t.equal(run.archive[archive].text, LOG)
	t.equal(run.archive[archive].mode, 384, "0o600")

	t.equal(#run.jev > 0 and #run.jev <= 12, true, tostring(#run.jev))
	t.equal(run.jev[1].state.command, "cargo build")
	t.equal(run.jev[1].state.task, "fix the build")
	local history = json.encode(run.jev[1].state.history)
	t.equal(contains(history, "fix the build"), true, history)
	t.equal(run.outcome.usage.cost.total > 0, true, "Jev's usage is charged")

	local reports = outputs(run)
	t.equal(#reports, 1)
	local report = reports[1]
	t.equal(report.pruned, true)
	t.equal(report.archive, archive)
	t.equal(type(report.tokens_after) == "number" and report.tokens_after < report.tokens_before, true)
	t.equal(report.dropped_lines > 0, true)
	t.equal(report.lines, 3000)
	-- Its cost is what the plugin charged for it.
	t.equal(run.charged > 0, true)
	t.equal(report.cost, run.charged)
	-- The call is not marked for the person's review.
	for _, event in ipairs(run.events) do
		local report = event.PluginReport
		t.equal(report ~= nil and report.body.kind == "flag", false, "a call is flagged")
	end
end)

-- A `write_stdin` result is pruned like an `exec_command` one; Jev is
-- told what was written.
t.loop(
	"a write_stdin result is pruned too",
	one_build(nil, {
		tools = { write_stdin = LOG },
		model = {
			{ tool = "write_stdin", args = { process_id = 7, chars = "y\n" } },
			{ text = "done" },
		},
	}),
	function(run)
		local seen = seen_result(run)
		t.equal(starts_with(seen, HEADER), true, seen)
		t.equal(contains(seen, "error[E0425]"), true, seen)
		t.equal(run.jev[1].state.command, "write_stdin: y\n")
		t.equal(outputs(run)[1].pruned, true)
	end
)

-- An output under the threshold, and a failed command however large,
-- reach the model as they were, without a Jev call.
local FAILED = LOG .. "\n\nCommand exited with code 101"
for _, case in ipairs({
	{ name = "a small output", exec = build_log(50), expected = build_log(50) },
	{ name = "a failed command", exec = { error = FAILED }, expected = FAILED },
}) do
	t.loop("small or failed outputs are untouched: " .. case.name, one_build(case.exec), function(run)
		t.equal(seen_result(run), case.expected)
		t.equal(#run.jev, 0)
		t.equal(#outputs(run), 0)
		t.equal(archived(run), {})
	end)
end

-- With output pruning off, a large output is untouched too.
t.loop("output pruning can be off", one_build(LOG, { settings = { output_enabled = false } }), function(run)
	t.equal(seen_result(run), LOG)
	t.equal(#run.jev, 0)
end)

-- A Jev failure leaves the output as it was, says so in the plugin's
-- log, and writes no archive.
t.loop("a jev failure leaves the output", one_build(LOG, { jev = { error = 503 } }), function(run)
	t.equal(run.outcome.text, "done")
	t.equal(seen_result(run), LOG)
	t.equal(#run.logs, 1, json.encode(run.logs))
	t.equal(contains(run.logs[1], "Jev answered with status 503"), true, run.logs[1])
	t.equal(archived(run), {})
end)

-- When Jev keeps everything, the output is untouched: nothing went, so
-- nothing would be saved; the state says so.
t.loop("keeping everything leaves the output", one_build(LOG, { jev = { { noul = 0.9 } } }), function(run)
	t.equal(seen_result(run), LOG)
	local reports = outputs(run)
	t.equal(#reports, 1)
	t.equal(reports[1].pruned, false)
	t.equal(reports[1].dropped_lines, 0)
	t.equal(archived(run), {})
end)
