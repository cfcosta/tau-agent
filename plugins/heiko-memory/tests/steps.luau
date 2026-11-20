-- Each memory tool's step (ADR 0064), from what its call gave: what the
-- demo replays as a run's calls, and what each line says.
local heiko = require("heiko")
local t = heiko.test
local memory = require("memory")

local ROOTS = { [memory.REPOSITORY] = "/repo/heiko-memory", [memory.DATA] = "/data/heiko-memory" }

local GOTCHA = {
	type = "gotcha",
	title = "Release tests time out when the test binary is built in debug",
	description = "Build tests with --release; debug binaries run the property tests too slowly",
	body = "The property tests of heiko-ui-remote take minutes in a debug build and hit nextest's timeout.\n\nRun `cargo nextest run --release`.",
}

-- The run's five calls, in order: each tool, its arguments, what the
-- model read, the data its step read, and the step.
local function calls()
	local run = t.run({ roots = ROOTS })
	local made = {}
	local function call(tool, args)
		local text, data = run:tool(tool, args, "call_" .. (#made + 1))
		table.insert(made, { tool = tool, args = args, text = text, value = data, step = run:step(tool, args, data) })
		return data
	end
	local written = call("memory_write", GOTCHA)
	call("memory_search", { query = "nextest timeout" })
	call("memory_read", { id = written.id })
	call("memory_attempt", {
		id = written.id,
		what = "Raise the slow-timeout to 300 s",
		outcome = "failed",
		evidence = "the hegel suite still timed out at 300 s",
	})
	call("memory_link", { from = written.id, to = "crates/heiko-ui-remote", type = "about", why = "its tests are the slow ones" })
	return made
end

t.case("each call's step names what it is about and what it did", function()
	local made = calls()
	local function said(at)
		local step = made[at].step
		local receipt = {}
		for _, part in ipairs(step.receipt) do
			table.insert(receipt, part.text or heiko.count.plural(part.n, part.noun))
		end
		return step.subject, table.concat(receipt, " · "), step.body and step.body.form or nil
	end
	t.equal({ said(1) }, { GOTCHA.title, "gotcha · created", "code" })
	t.equal({ said(2) }, { "nextest timeout", "1 note", "names" })
	t.equal({ said(3) }, { made[1].value.id, "9 lines", "code" })
	t.equal({ said(4) }, { made[1].value.id, "failed", "record" })
	t.equal({ said(5) }, { made[1].value.id .. " → crates/heiko-ui-remote", "about", "record" })
	t.equal(made[4].step.receipt[1].tone, "bad")
end)

-- A note a secret was taken out of shows neither its title nor its body.
t.case("a redacted note's step shows only its id", function()
	local run = t.run({ roots = ROOTS })
	local args = {
		type = "fact",
		title = "The deploy token",
		description = "Where the token is",
		body = "The token is ghp_" .. string.rep("a", 36) .. ".",
	}
	local _, data = run:tool("memory_write", args)
	local step = run:step("memory_write", args, data)
	t.equal(step.subject, data.id)
	t.equal(step.body, nil)
end)

t.fixture("steps", calls)
