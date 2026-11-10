-- The demo's canvases (ADR 0053): what the plugin leaves and draws of
-- the demo's run, which showed a chart, wrote a plan, and edited it.
local tau = require("tau")
local t = tau.test

-- The blob the demo's fixtures name for its chart: the demo keeps the
-- chart and puts the blob it got in its place.
local DEMO_BLOB = "demo-chart"

local PLAN = "## What changes\n\n"
	.. "- `canvas_show` keeps a copy of the image in the repository's blobs, so "
	.. "it still shows once the file changes.\n"
	.. "- `canvas_write` and `canvas_edit` keep every revision of a document.\n"
	.. "- The pane opens beside the transcript when something new arrives.\n\n"
	.. "## Open questions\n\n"
	.. "1. Should the person edit a document in place?\n"
	.. "2. Should uploads land on the canvas too?\n"

local CAPTION = "p50 and p99 latency per attempt, before and after the backoff change"

-- The demo's run: the calls as the demo replays them, and the canvas
-- they leave, picked as `pick` says, with what the plugin draws of it.
local function demo(pick)
	local image = {
		blob = { id = DEMO_BLOB, digest = DEMO_BLOB, size_bytes = 0 },
		mime = "image/png",
		width = 1200,
		height = 640,
		path = "target/plots/retry-latency.png",
	}
	-- A chart only a real file gives: as canvas_show leaves it.
	-- The demo's run is in the repository tau-agent.
	local r = t.run({
		id = "retry-after",
		repo = "tau-agent",
		state = {
			canvas = {
				items = {
					{
						id = 1,
						title = "Retry latency",
						revisions = {
							{
								call = "canvas-show",
								content = { type = "image", image = image, caption = CAPTION },
							},
						},
					},
				},
				fresh = 1,
				last = 1,
			},
		},
	})
	local calls = {
		{
			id = "canvas-show",
			tool = "canvas_show",
			args = { path = "target/plots/retry-latency.png", title = "Retry latency", caption = CAPTION },
			text = 'Image 1, "Retry latency", is on the canvas (1200×640 PNG).',
		},
	}
	for _, step in
		{
			{ "canvas-write", "canvas_write", { title = "Canvas plan", text = PLAN } },
			{
				"canvas-edit",
				"canvas_edit",
				{
					item = 2,
					edits = {
						{
							old = "2. Should uploads land on the canvas too?",
							new = "2. Uploads land on the canvas too: the person's images go to the model as input.",
						},
					},
				},
			},
		}
	do
		local id, tool, args = step[1], step[2], step[3]
		local text = r:tool(tool, table.clone(args), id)
		table.insert(calls, { id = id, tool = tool, args = args, text = text })
	end
	if pick then
		r:action("pick", { item = pick[1], revision = pick[2], fresh = r.state.canvas.fresh })
	end
	local view = r:view()
	return { blob = DEMO_BLOB, calls = calls, state = r.state, view = view }
end

t.fixture("canvas", function()
	return demo(nil)
end)

t.fixture("canvas-image", function()
	return demo({ 1, 1 })
end)

t.fixture("canvas-document", function()
	return demo({ 2, 1 })
end)
