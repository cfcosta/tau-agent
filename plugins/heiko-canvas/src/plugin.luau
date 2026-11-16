-- heiko-canvas: what a run makes for the person to look at, in a pane
-- beside its transcript.
--
-- The model shows an image file with `canvas_show`, writes a Markdown
-- document with `canvas_write`, revises one with `canvas_edit`, and reads
-- one back with `canvas_read`. Each image or document is an item on the
-- run's canvas, numbered from 1, and every revision of it is kept: the
-- person can step back through them. The canvas is the run's state; an
-- image's bytes are a blob of the repository's, which the interface asks
-- for once.
local heiko = require("heiko")
local ui = heiko.ui
local cmd = heiko.cmd
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
local function revise(model, env, id, title, content, said)
	local of = canvas.of(model)
	canvas.revise(of, env.run.call, id, title, content)
	model.canvas = of
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

local function show(args, model, env)
	only(args, { path = true, title = true, caption = true, item = true })
	local path, title = tostring(args.path), tostring(args.title)
	-- The host's own words when it cannot read the file, for the model.
	local read_ok, info = pcall(heiko.await, cmd.files.image(path))
	if not read_ok then
		error(info, 0)
	end
	if info.size > canvas.MAX_IMAGE_BYTES then
		error(
			path .. " is " .. info.size .. " bytes; the canvas takes images of at most " .. canvas.MAX_IMAGE_BYTES .. " bytes.",
			0
		)
	end
	if info.mime == nil then
		error(path .. " is not a PNG, JPEG, GIF, WebP, BMP or SVG image.", 0)
	end
	if args.item ~= nil then
		existing(canvas.of(model), args.item, "image")
	end
	local ok, blob = pcall(heiko.await, cmd.blobs.put_file(path))
	if not ok then
		error("The canvas could not keep it: " .. tostring(blob), 0)
	end
	-- The canvas as it is now: another call may have revised it while the
	-- blob was kept, so the item is looked up again.
	local of = canvas.of(model)
	if args.item ~= nil then
		existing(of, args.item, "image")
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
	return revise(model, env, id, title, { type = "image", image = image, caption = args.caption or "" }, said)
end

local function write(args, model, env)
	only(args, { title = true, text = true, item = true })
	local title, text = tostring(args.title), tostring(args.text)
	check_size(text)
	local of = canvas.of(model)
	if args.item ~= nil then
		existing(of, args.item, "document")
	end
	local id = args.item or canvas.next_id(of)
	local item = canvas.item(of, id)
	local revision = item and #item.revisions + 1 or 1
	local said = revision == 1 and ("Document " .. id .. ', "' .. title .. '", is on the canvas.')
		or ("Document " .. id .. ', "' .. title .. '", is at revision ' .. revision .. ".")
	return revise(model, env, id, title, { type = "document", text = text }, said)
end

local function edit(args, model, env)
	only(args, { item = true, edits = true, title = true })
	local of = canvas.of(model)
	local item = existing(of, args.item, "document")
	local text, why = canvas.apply_edits(canvas.latest(item).content.text, args.edits or {})
	if text == nil then
		error(why, 0)
	end
	check_size(text)
	local title = args.title or item.title
	local revision = #item.revisions + 1
	local said = "Document " .. args.item .. ', "' .. title .. '", is at revision ' .. revision .. "."
	return revise(model, env, args.item, title, { type = "document", text = text }, said)
end

local function read(args, model)
	only(args, { item = true, revision = true })
	local of = canvas.of(model)
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
local function picture(image, env, height)
	return ui.image(image.blob, image.mime, env.run.repo or "", height)
end

-- A word that shows the revision `revision` of `item`.
local function pick_button(label, of, item, revision)
	return ui.button(label, "pick", { item = item.id, revision = revision, fresh = of.fresh }, "secondary")
end

-- A tab per item, a dot between each two: the one shown in bold, the
-- others a click away.
local function tabs(of, shown)
	local row = {}
	for at, item in ipairs(of.items) do
		if at > 1 then
			table.insert(row, ui.toned("·", "neutral"))
		end
		if item.id == shown then
			table.insert(row, ui.rich("**" .. item.title .. "**"))
		else
			table.insert(row, pick_button(item.title, of, item, #item.revisions))
		end
	end
	return ui.row(row)
end

-- Which revision is shown: a square for each, the one shown filled, and
-- the ones before and after it a click away.
local function revisions(of, item, revision)
	local count = #item.revisions
	local function step(label, to)
		if to == nil then
			return ui.toned(label, "neutral")
		end
		return pick_button(label, of, item, to)
	end
	local bar = {}
	for at = 1, count do
		table.insert(bar, ui.square(tostring(at), at == revision and "filled" or "quiet", "info"))
	end
	local noun = canvas.kind(item) == "image" and "Version" or "Revision"
	table.insert(bar, ui.text(noun .. " " .. revision .. " of " .. count))
	if revision < count then
		table.insert(bar, ui.toned("not the latest", "warn"))
	end
	table.insert(bar, ui.spacer())
	table.insert(bar, step("older", revision > 1 and revision - 1 or nil))
	table.insert(bar, step("newer", revision < count and revision + 1 or nil))
	return ui.row(bar)
end

-- The pane: a tab per item when there are several, the revision shown
-- with a way through the others, and the image or the document itself.
local function pane(of, pick, env)
	local item, revision = canvas.shown(of, pick)
	if item == nil then
		return nil
	end
	local bars = {}
	if #of.items > 1 then
		table.insert(bars, tabs(of, item.id))
		table.insert(bars, ui.rule())
	end
	if #item.revisions > 1 then
		table.insert(bars, revisions(of, item, revision))
	end
	local content = item.revisions[revision].content
	if content.type == "image" then
		-- The image over a rule, then what it shows and what it is.
		local parts = { picture(content.image, env, IMAGE_HEIGHT), ui.rule() }
		if content.caption ~= "" then
			table.insert(parts, ui.text(content.caption))
		end
		table.insert(parts, ui.toned(canvas.describe(content.image) .. " · " .. content.image.path, "neutral", true))
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
local function card(of, item, revision, env)
	local content = item.revisions[revision].content
	local label
	if content.type == "image" then
		label = canvas.describe(content.image)
	else
		label = "revision " .. revision .. " · " .. canvas.grouped(canvas.words(content.text)) .. " words"
	end
	local open = ui.button("open on the canvas", "open", { item = item.id, revision = revision, fresh = of.fresh })
	local body
	if content.type == "image" then
		body = ui.stack({ picture(content.image, env, THUMBNAIL), open })
	else
		body = ui.stack({ ui.stack(canvas.excerpt(content.text, EXCERPT_LINES)), open })
	end
	return {
		head = ui.mono(canvas.kind(item) .. " " .. item.id .. " · " .. item.title),
		label = label,
		body = body,
	}
end

return heiko.plugin({
	uses = { files = { "workspace", "~" } },

	tools = {
		canvas_show = {
			kind = "change",
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
			kind = "change",
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
			kind = "change",
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
			kind = "look",
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
		pick = function(args, model)
			model.pick = { item = args.item, revision = args.revision, fresh = args.fresh }
		end,
		open = function(args, model)
			model.pick = { item = args.item, revision = args.revision, fresh = args.fresh }
			return heiko.interface("pane")
		end,
	},

	-- The pane, once the run has anything on its canvas; each call's card;
	-- and how many items there are, on the status line.
	view = function(model, env)
		local of = canvas.of(model)
		if #of.items == 0 then
			return {}
		end
		local calls = {}
		for _, item in ipairs(of.items) do
			for at, revision in ipairs(item.revisions) do
				calls[revision.call] = card(of, item, at, env)
			end
		end
		local count = #of.items
		return {
			pane = pane(of, model.pick, env),
			calls = calls,
			status = ui.status(heiko.count.plural(count, "item on the canvas", "items on the canvas")),
		}
	end,
})
