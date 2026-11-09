-- tau-canvas: what a run makes for the person to look at, in a pane
-- beside its transcript.
--
-- The model shows an image file with `canvas_show`, writes a Markdown
-- document with `canvas_write`, revises one with `canvas_edit`, and reads
-- one back with `canvas_read`. Each image or document is an item on the
-- run's canvas, numbered from 1, and every revision of it is kept: the
-- person can step back through them. The canvas is the run's state; an
-- image's bytes are a blob of the repository's, which the interface asks
-- for once.
local tau = require("tau")
local ui = tau.ui
local canvas = require("canvas")

-- How tall a document scrolls, at most, and an image is, in the pane;
-- and an image's thumbnail on its card. In rems.
local DOCUMENT_HEIGHT = 80
local IMAGE_HEIGHT = 30
local THUMBNAIL = 10

-- The lines of a document a card shows.
local EXCERPT_LINES = 6

local function article(kind)
	return kind == "image" and "an" or "a"
end

-- The item `id` must be there, and be of `kind`.
local function existing(of, id, kind)
	local item = canvas.item(of, id)
	if item == nil then
		error("There is no item " .. id .. " on the canvas.", 0)
	end
	local is = canvas.kind(item)
	if is ~= kind then
		error("Item " .. id .. " is " .. article(is) .. " " .. is .. ", not " .. article(kind) .. " " .. kind .. ".", 0)
	end
	return item
end

-- The arguments a tool takes, by name: any other is refused.
local function only(args, fields)
	for field in pairs(args) do
		if not fields[field] then
			error("unknown field `" .. tostring(field) .. "`", 0)
		end
	end
end

-- The revision `content` of item `id`, made by this call, on the run's
-- canvas; `said` is what the model reads.
local function revise(ctx, id, title, content, said)
	local of = canvas.of(ctx.state)
	canvas.revise(of, ctx.run.call, id, title, content)
	ctx.state.canvas = of
	return said
end

local function check_size(text)
	if #text > canvas.MAX_DOCUMENT_BYTES then
		error(
			"The document is "
				.. #text
				.. " bytes; the canvas takes documents of at most "
				.. canvas.MAX_DOCUMENT_BYTES
				.. " bytes.",
			0
		)
	end
end

local function show(args, ctx)
	only(args, { path = true, title = true, caption = true, item = true })
	local path, title = tostring(args.path), tostring(args.title)
	local info = ctx.files.image(path)
	if info.size > canvas.MAX_IMAGE_BYTES then
		error(
			path .. " is " .. info.size .. " bytes; the canvas takes images of at most " .. canvas.MAX_IMAGE_BYTES .. " bytes.",
			0
		)
	end
	if info.mime == nil then
		error(path .. " is not a PNG, JPEG, GIF, WebP, BMP or SVG image.", 0)
	end
	local of = canvas.of(ctx.state)
	if args.item ~= nil then
		existing(of, args.item, "image")
	end
	local ok, blob = pcall(ctx.blobs.put_file, path)
	if not ok then
		error("The canvas could not keep it: " .. tostring(blob), 0)
	end
	local image = { blob = blob, mime = info.mime, width = info.width, height = info.height, path = path }
	local id = args.item or canvas.next_id(of)
	local item = canvas.item(of, id)
	local revision = item and #item.revisions + 1 or 1
	local said
	if revision == 1 then
		said = "Image " .. id .. ', "' .. title .. '", is on the canvas (' .. canvas.describe(image) .. ")."
	else
		said = "Image "
			.. id
			.. ', "'
			.. title
			.. '", shows version '
			.. revision
			.. " ("
			.. canvas.describe(image)
			.. ")."
	end
	return revise(ctx, id, title, { type = "image", image = image, caption = args.caption or "" }, said)
end

local function write(args, ctx)
	only(args, { title = true, text = true, item = true })
	local title, text = tostring(args.title), tostring(args.text)
	check_size(text)
	local of = canvas.of(ctx.state)
	if args.item ~= nil then
		existing(of, args.item, "document")
	end
	local id = args.item or canvas.next_id(of)
	local item = canvas.item(of, id)
	local revision = item and #item.revisions + 1 or 1
	local said = revision == 1 and ("Document " .. id .. ', "' .. title .. '", is on the canvas.')
		or ("Document " .. id .. ', "' .. title .. '", is at revision ' .. revision .. ".")
	return revise(ctx, id, title, { type = "document", text = text }, said)
end

local function edit(args, ctx)
	only(args, { item = true, edits = true, title = true })
	local of = canvas.of(ctx.state)
	local item = existing(of, args.item, "document")
	local text, why = canvas.apply_edits(canvas.latest(item).content.text, args.edits or {})
	if text == nil then
		error(why, 0)
	end
	check_size(text)
	local title = args.title or item.title
	local revision = #item.revisions + 1
	local said = "Document " .. args.item .. ', "' .. title .. '", is at revision ' .. revision .. "."
	return revise(ctx, args.item, title, { type = "document", text = text }, said)
end

local function read(args, ctx)
	only(args, { item = true, revision = true })
	local of = canvas.of(ctx.state)
	local item = canvas.item(of, args.item)
	if item == nil then
		error("There is no item " .. args.item .. " on the canvas.", 0)
	end
	local count = #item.revisions
	local revision = args.revision or count
	local found = revision >= 1 and item.revisions[revision] or nil
	if found == nil then
		error("Item " .. args.item .. " has revisions 1 to " .. count .. ".", 0)
	end
	local content = found.content
	if content.type == "document" then
		return content.text
	end
	local caption = content.caption ~= "" and (". " .. content.caption) or ""
	return "Image "
		.. args.item
		.. ', "'
		.. item.title
		.. '": '
		.. canvas.format_name(content.image.mime)
		.. " from "
		.. content.image.path
		.. caption
end

-- The image `image` of the run's repository as a piece, `height` rems
-- tall.
local function picture(image, ctx, height)
	return ui.image(image.blob, image.mime, ctx.run.repo or "", height)
end

-- A button that shows the revision `revision` of `item`.
local function pick_button(label, of, item, revision, kind, icon)
	return ui.button(label, "pick", { item = item.id, revision = revision, fresh = of.fresh }, kind, icon)
end

-- A tab per item, the one shown marked; a click shows another.
local function tabs(of, shown)
	local row = {}
	for _, item in ipairs(of.items) do
		local noun = canvas.kind(item) == "image" and "Image ·" or "Document ·"
		local kind = item.id == shown and "primary" or "secondary"
		table.insert(row, pick_button(noun .. " " .. item.title, of, item, #item.revisions, kind))
	end
	return ui.row(row)
end

-- Which revision is shown, of how many, with the ones before and after
-- it a click away.
local function revisions(of, item, revision)
	local count = #item.revisions
	local function step(glyph, to)
		if to == nil then
			return ui.icon(glyph, "neutral")
		end
		return pick_button("", of, item, to, "secondary", glyph)
	end
	local noun = canvas.kind(item) == "image" and "Version" or "Revision"
	local bar = {
		step("back", revision > 1 and revision - 1 or nil),
		step("chevron", revision < count and revision + 1 or nil),
		ui.text(noun .. " " .. revision .. " of " .. count),
	}
	if revision < count then
		table.insert(bar, ui.text("· not the latest"))
	end
	return ui.row(bar)
end

-- The pane: a tab per item when there are several, the revision shown
-- with a way through the others, and the image or the document itself.
local function pane(of, pick, ctx)
	local item, revision = canvas.shown(of, pick)
	if item == nil then
		return nil
	end
	local bars = {}
	if #of.items > 1 then
		table.insert(bars, tabs(of, item.id))
	end
	if #item.revisions > 1 then
		table.insert(bars, revisions(of, item, revision))
	end
	local content = item.revisions[revision].content
	if content.type == "image" then
		local parts = { picture(content.image, ctx, IMAGE_HEIGHT) }
		if content.caption ~= "" then
			table.insert(parts, ui.text(content.caption))
		end
		table.insert(parts, ui.mono(canvas.describe(content.image) .. " · " .. content.image.path))
		table.insert(bars, ui.stack(parts))
	else
		table.insert(
			bars,
			ui.scroll({ { piece = "title", text = item.title }, ui.markdown(content.text) }, DOCUMENT_HEIGHT)
		)
	end
	return { title = "Canvas", fresh = of.fresh, body = ui.stack(bars) }
end

-- What a call's card shows of the revision it made: what it is, and a
-- way to open it on the canvas.
local function card(of, item, revision, ctx)
	local content = item.revisions[revision].content
	local label
	if content.type == "image" then
		label = canvas.describe(content.image)
	else
		label = "revision " .. revision .. " · " .. canvas.grouped(canvas.words(content.text)) .. " words"
	end
	local open = ui.button("Open on the canvas", "open", { item = item.id, revision = revision, fresh = of.fresh })
	local body
	if content.type == "image" then
		body = ui.stack({ picture(content.image, ctx, THUMBNAIL), open })
	else
		body = ui.stack({ ui.card(canvas.excerpt(content.text, EXCERPT_LINES)), open })
	end
	return {
		head = ui.mono(canvas.kind(item) .. " " .. item.id .. " · " .. item.title),
		label = label,
		body = body,
	}
end

return tau.plugin({
	uses = { files = { "workspace", "~" } },

	tools = {
		canvas_show = {
			description = "Shows the person an image file on the canvas, a pane beside the conversation. Use it for a picture they should look at: a chart you plotted, a screenshot, a diagram you rendered. The file must be PNG, JPEG, GIF, WebP, BMP or SVG, at most 8 MiB. The canvas keeps a copy, so the file may change afterwards. Give `item` to replace an image on the canvas with a new version; leave it out to add one.",
			parameters = {
				type = "object",
				properties = {
					path = {
						type = "string",
						description = "The image file: relative to the workspace, absolute, or from home with `~/`.",
					},
					title = { type = "string", description = "A short title for the image." },
					caption = { type = "string", description = "A line under the image saying what it shows." },
					item = {
						type = "integer",
						minimum = 0,
						description = "The image on the canvas to replace with a new version, by its number. Leave it out to add a new image.",
					},
				},
				required = { "path", "title" },
				additionalProperties = false,
			},
			call = show,
		},
		canvas_write = {
			description = "Writes a Markdown document on the canvas, a pane beside the conversation, for the person to read: a plan, a report, a draft, anything longer than a reply that they will want to keep open while you talk. Give `item` to replace a document's whole text with a new revision; leave it out to start a new document. To change part of a document, use canvas_edit.",
			parameters = {
				type = "object",
				properties = {
					title = { type = "string", description = "The document's title." },
					text = { type = "string", description = "The whole document, in Markdown." },
					item = {
						type = "integer",
						minimum = 0,
						description = "The document on the canvas whose text this replaces, as a new revision, by its number. Leave it out to start a new document.",
					},
				},
				required = { "title", "text" },
				additionalProperties = false,
			},
			call = write,
		},
		canvas_edit = {
			description = "Revises a document on the canvas by exact replacements: each `old` must occur exactly once in the text as the edits before it left it, and becomes `new`. The result is a new revision; the person can step back through earlier ones.",
			parameters = {
				type = "object",
				properties = {
					item = { type = "integer", minimum = 0, description = "The document to revise, by its number." },
					edits = {
						type = "array",
						description = "Replacements made in turn.",
						items = {
							type = "object",
							description = "An exact replacement in a document.",
							properties = {
								old = {
									type = "string",
									description = "Text that occurs exactly once in the document, as the edits before this one left it.",
								},
								new = { type = "string", description = "What replaces it." },
							},
							required = { "old", "new" },
						},
					},
					title = { type = "string", description = "A new title, if it changes." },
				},
				required = { "item", "edits" },
				additionalProperties = false,
			},
			call = edit,
		},
		canvas_read = {
			description = "Reads a document's text from the canvas: its latest revision, or `revision`, numbered from 1. An image reads as its title, caption and file.",
			parameters = {
				type = "object",
				properties = {
					item = { type = "integer", minimum = 0, description = "The document to read, by its number." },
					revision = {
						type = "integer",
						minimum = 0,
						description = "The revision to read, from 1. Leave it out for the latest.",
					},
				},
				required = { "item" },
				additionalProperties = false,
			},
			call = read,
		},
	},

	-- A tree's button: show the revision its arguments pick, and with
	-- `open`, open the pane too.
	actions = {
		pick = function(args, ctx)
			ctx.state.pick = { item = args.item, revision = args.revision, fresh = args.fresh }
		end,
		open = function(args, ctx)
			ctx.state.pick = { item = args.item, revision = args.revision, fresh = args.fresh }
			return tau.interface("pane")
		end,
	},

	-- The pane, once the run has anything on its canvas; each call's card;
	-- and how many items there are, on the status line.
	view = function(state, ctx)
		local of = canvas.of(state)
		if #of.items == 0 then
			return {}
		end
		local calls = {}
		for _, item in ipairs(of.items) do
			for at, revision in ipairs(item.revisions) do
				calls[revision.call] = card(of, item, at, ctx)
			end
		end
		local count = #of.items
		return {
			pane = pane(of, state.pick, ctx),
			calls = calls,
			status = ui.status(count == 1 and "1 item on the canvas" or (count .. " items on the canvas")),
		}
	end,
})
