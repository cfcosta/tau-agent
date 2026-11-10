-- tau-canvas in a real run (ADR 0050, gap G): `canvas_show` reads an
-- image from the run's workspace and keeps it as a blob of the
-- repository's, which reads back whole; a file past the folders the
-- plugin declares is not read.
local tau = require("tau")
local t = tau.test

-- A run whose model shows `args`, then answers.
local function show(args, files)
	return {
		files = files,
		model = {
			{ tool = "canvas_show", args = args },
			{ text = "Shown." },
		},
	}
end

t.loop(
	"an image from the workspace is kept and read back whole",
	show({ path = "plot.png", title = "Plot" }, { ["plot.png"] = t.png(12, 8) }),
	function(run)
		t.equal(run.results[1].text, 'Image 1, "Plot", is on the canvas (12×8 PNG).')
		local blob = run.state.canvas.items[1].revisions[1].content.image.blob
		t.equal(run.blobs[blob.id], run.workspace["plot.png"])
	end
)

-- `/etc/passwd` is neither the run's workspace nor under `~`.
t.loop("a file past its folders is not shown", show({ path = "/etc/passwd", title = "x" }), function(run)
	t.equal(run.results[1].error, true)
	local said = run.results[1].text
	t.equal(string.find(said, "its folder is not in `uses.files`", 1, true) ~= nil, true, said)
end)
