-- tau-canvas in a run: images and documents land on the canvas with
-- every revision, and read back; a call asked wrongly says why and
-- changes nothing; and the pane, the cards and the status show it all,
-- the pane's buttons picking what it shows.
local tau = require("tau")
local t = tau.test

local PNG = { text = "png bytes", size = 1200, image = { mime = "image/png", width = 12, height = 8 } }
local SVG = { text = "<svg/>", image = { mime = "image/svg+xml" } }

local function run(spec)
	spec = spec or {}
	spec.files = spec.files or {
		["plot.png"] = PNG,
		["chart.svg"] = SVG,
		["notes.txt"] = "not an image",
		["huge.png"] = { size = 8 * 1024 * 1024 + 1, image = { mime = "image/png" } },
	}
	return t.run(spec)
end

-- Why `run:tool(name, args)` failed; it must fail.
local function refused(r, name, args)
	local ok, why = pcall(r.tool, r, name, args)
	t.equal(ok, false, "expected " .. name .. " to refuse")
	return why
end

t.case("images and documents land on the canvas with every revision", function()
	local r = run()
	t.equal(
		r:tool("canvas_show", { path = "plot.png", title = "Plot", caption = "Loss" }, "c1"),
		'Image 1, "Plot", is on the canvas (12×8 PNG).'
	)
	t.equal(
		r:tool("canvas_write", { title = "Plan", text = "# Plan\nFirst, measure." }, "c2"),
		'Document 2, "Plan", is on the canvas.'
	)
	t.equal(
		r:tool("canvas_edit", { item = 2, edits = { { old = "measure", new = "profile" } } }, "c3"),
		'Document 2, "Plan", is at revision 2.'
	)
	t.equal(
		r:tool("canvas_show", { path = "chart.svg", title = "Chart", item = 1 }, "c4"),
		'Image 1, "Chart", shows version 2 (SVG).'
	)
	t.equal(
		r:tool("canvas_write", { title = "Plan v3", text = "Done.", item = 2 }, "c5"),
		'Document 2, "Plan v3", is at revision 3.'
	)
	t.equal(r:tool("canvas_read", { item = 2 }), "Done.")
	t.equal(r:tool("canvas_read", { item = 2, revision = 2 }), "# Plan\nFirst, profile.")
	t.equal(r:tool("canvas_read", { item = 1, revision = 1 }), 'Image 1, "Chart": PNG from plot.png. Loss')
	t.equal(r:tool("canvas_read", { item = 1 }), 'Image 1, "Chart": SVG from chart.svg')
	local items = r.state.canvas.items
	t.equal(#items, 2)
	t.equal(#items[1].revisions, 2)
	t.equal(items[1].revisions[1].content.image.blob.id, "plot.png")
	t.equal(r.state.canvas.fresh, 5)
	t.equal(r.state.canvas.last, 2)
end)

t.case("a call asked wrongly says why and changes nothing", function()
	local r = run()
	r:tool("canvas_write", { title = "Plan", text = "one" }, "c1")
	local before = r.state.canvas
	t.equal(
		refused(r, "canvas_show", { path = "notes.txt", title = "x" }),
		"notes.txt is not a PNG, JPEG, GIF, WebP, BMP or SVG image."
	)
	t.equal(
		refused(r, "canvas_show", { path = "huge.png", title = "x" }),
		"huge.png is 8388609 bytes; the canvas takes images of at most 8388608 bytes."
	)
	t.equal(
		refused(r, "canvas_show", { path = "gone.png", title = "x" }),
		"Cannot read gone.png: No such file or directory (os error 2)"
	)
	t.equal(refused(r, "canvas_show", { path = "plot.png", title = "x", item = 1 }), "Item 1 is a document, not an image.")
	t.equal(refused(r, "canvas_write", { title = "x", text = "y", item = 7 }), "There is no item 7 on the canvas.")
	t.equal(
		refused(r, "canvas_edit", { item = 1, edits = { { old = "two", new = "2" } } }),
		"Edit 1: `old` does not occur in the document."
	)
	t.equal(
		refused(r, "canvas_write", { title = "x", text = string.rep("a", 256 * 1024 + 1) }),
		"The document is 262145 bytes; the canvas takes documents of at most 262144 bytes."
	)
	t.equal(refused(r, "canvas_read", { item = 1, revision = 2 }), "Item 1 has revisions 1 to 1.")
	t.equal(refused(r, "canvas_read", { item = 1, revision = 0 }), "Item 1 has revisions 1 to 1.")
	t.equal(refused(r, "canvas_write", { title = "x", text = "y", page = 2 }), "unknown field `page`")
	t.equal(r.state.canvas, before)
end)

-- Every piece in `tree` whose `piece` is `kind`, in order.
local function find(tree, kind, found)
	found = found or {}
	if type(tree) == "table" then
		if tree.piece == kind then
			table.insert(found, tree)
		end
		for key, value in pairs(tree) do
			if key ~= "piece" then
				find(value, kind, found)
			end
		end
	end
	return found
end

t.case("nothing on the canvas draws nothing", function()
	t.equal(run():view(), {})
end)

t.case("the pane shows the item revised last, and its buttons pick another", function()
	local r = run({ id = "r1" })
	r:tool("canvas_show", { path = "plot.png", title = "Plot", caption = "Loss" }, "c1")
	r:tool("canvas_write", { title = "Plan", text = "a" }, "c2")
	r:tool("canvas_write", { title = "Plan", text = "b", item = 2 }, "c3")
	local view = r:view()
	t.equal(view.pane.title, "Canvas")
	t.equal(view.pane.fresh, 3)
	t.equal(view.status.text, "2 items on the canvas")
	local buttons = find(view.pane.body, "button")
	-- A tab per item, the one shown marked, and the step back.
	t.equal(buttons[1].label, "Image · Plot")
	t.equal(buttons[1].kind, "secondary")
	t.equal(buttons[2].label, "Document · Plan")
	t.equal(buttons[2].kind, "primary")
	t.equal(buttons[3].icon, "back")
	t.equal(find(view.pane.body, "markdown")[1].text, "b")
	-- Back a revision: it says it is not the latest.
	r:action("pick", buttons[3].args)
	view = r:view()
	t.equal(find(view.pane.body, "markdown")[1].text, "a")
	local texts = {}
	for _, piece in ipairs(find(view.pane.body, "text")) do
		table.insert(texts, piece.text)
	end
	t.equal(texts, { "Revision 1 of 2", "· not the latest" })
	-- The image tab: the image, its caption, what it is and where from.
	r:action("pick", find(view.pane.body, "button")[1].args)
	view = r:view()
	local image = find(view.pane.body, "image")[1]
	t.equal(image.key, "plot.png")
	t.equal(image.mime, "image/png")
	t.equal(image.height, 30)
	t.equal(find(view.pane.body, "mono")[1].text, "12×8 PNG · plot.png")
	-- Something newer comes: it is shown again.
	r:tool("canvas_write", { title = "Plan", text = "c", item = 2 }, "c4")
	t.equal(find(r:view().pane.body, "markdown")[1].text, "c")
end)

t.case("each call's card shows its revision and opens it on the canvas", function()
	local r = run()
	r:tool("canvas_show", { path = "plot.png", title = "Plot" }, "c1")
	r:tool("canvas_write", { title = "Plan", text = "# Plan\n\nOne two `three`." }, "c2")
	local calls = r:view().calls
	t.equal(calls.c1.head, { piece = "mono", text = "image 1 · Plot" })
	t.equal(calls.c1.label, "12×8 PNG")
	t.equal(find(calls.c1.body, "image")[1].height, 10)
	t.equal(calls.c2.label, "revision 1 · 5 words")
	local open = find(calls.c2.body, "button")[1]
	t.equal(open.label, "Open on the canvas")
	t.equal(r:action("open", open.args), tau.interface("pane"))
	t.equal(r.state.pick, { item = 2, revision = 1, fresh = 2 })
	local lines = {}
	for _, piece in ipairs(find(calls.c2.body, "text")) do
		table.insert(lines, piece.text)
	end
	t.equal(lines, { "Plan", "One two three." })
end)
