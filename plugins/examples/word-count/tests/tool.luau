local heiko = require("heiko")
local ui = heiko.ui
local t = heiko.test

-- `tools` fakes what the tool calls: a value, or a function of the
-- arguments.
local function run()
	return t.run({
		tools = {
			read = function(args)
				t.equal(args.path, "notes.md")
				return "one two  three\nfour"
			end,
		},
	})
end

t.case("counts the words read", function()
	t.equal(run():tool("word_count", { path = "notes.md" }), { path = "notes.md", words = 4 })
end)

t.case("the step counts the words", function()
	local step = run():step("word_count", { path = "notes.md" }, { path = "notes.md", words = 4 })
	t.equal(step, { subject = "notes.md", receipt = { ui.receipt.count(4, "word") } })
end)

t.case("the status line counts the files", function()
	local r = run()
	t.equal(r:view(), {})
	r:tool("word_count", { path = "notes.md" })
	r:tool("word_count", { path = "notes.md" })
	t.equal(r.state, { counted = 2 })
	t.equal(r:view().status, ui.status("counted", "2 files"))
end)
