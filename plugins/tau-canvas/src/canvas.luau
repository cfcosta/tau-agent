-- A run's canvas: its items, each an image or a document with every
-- revision of it, numbered from 1 as the model names them; the edits
-- that revise a document; and which revision the pane shows.
local canvas = {}

-- The largest image the canvas takes: 8 MiB, so its bytes reach a phone
-- in one message, base64 and all.
canvas.MAX_IMAGE_BYTES = 8 * 1024 * 1024

-- The longest document the canvas takes: 256 KiB of text. Every revision
-- travels with the run's state to each interface.
canvas.MAX_DOCUMENT_BYTES = 256 * 1024

-- A canvas from what `state.canvas` holds: `{ items, fresh, last }`.
function canvas.of(state)
	local kept = state.canvas or {}
	return { items = kept.items or {}, fresh = kept.fresh or 0, last = kept.last }
end

-- The item numbered `id`, or nil.
function canvas.item(of, id)
	for _, item in ipairs(of.items) do
		if item.id == id then
			return item
		end
	end
	return nil
end

-- The number the next new item gets.
function canvas.next_id(of)
	local most = 0
	for _, item in ipairs(of.items) do
		most = math.max(most, item.id)
	end
	return most + 1
end

-- An item's latest revision, and what it is: "image" or "document".
function canvas.latest(item)
	return item.revisions[#item.revisions]
end

function canvas.kind(item)
	return canvas.latest(item).content.type
end

-- The revision of the call `call`: its item, and its place among the
-- item's revisions, from 1.
function canvas.by_call(of, call)
	for _, item in ipairs(of.items) do
		for at, revision in ipairs(item.revisions) do
			if revision.call == call then
				return item, at
			end
		end
	end
	return nil
end

-- Folds a revision in: a new revision of its item, or a new item.
function canvas.revise(of, call, id, title, content)
	local revision = { call = call, content = content }
	local item = canvas.item(of, id)
	if item ~= nil then
		item.title = title
		table.insert(item.revisions, revision)
	else
		table.insert(of.items, { id = id, title = title, revisions = { revision } })
	end
	of.fresh += 1
	of.last = id
end

-- `text` with `edits` made in turn: each `old` must occur exactly once
-- in the text the edits before it left. Why not, when an edit fails.
function canvas.apply_edits(text, edits)
	if #edits == 0 then
		return nil, "No edits were given."
	end
	for n, edit in ipairs(edits) do
		local old, new = tostring(edit.old or ""), tostring(edit.new or "")
		if old == "" then
			return nil, "Edit " .. n .. ": `old` is empty."
		end
		local at = string.find(text, old, 1, true)
		if at == nil then
			return nil, "Edit " .. n .. ": `old` does not occur in the document."
		end
		if string.find(text, old, at + #old, true) ~= nil then
			return nil,
				"Edit "
					.. n
					.. ": `old` occurs more than once in the document; include more of the text around it."
		end
		text = string.sub(text, 1, at - 1) .. new .. string.sub(text, at + #old)
	end
	return text
end

-- Words in `text`, for a document's card.
function canvas.words(text)
	local count = 0
	for _ in string.gmatch(text, "%S+") do
		count += 1
	end
	return count
end

-- `1234` as `1,234`.
function canvas.grouped(count)
	local digits = tostring(count)
	local out = ""
	for index = 1, #digits do
		if index > 1 and (#digits - index + 1) % 3 == 0 then
			out ..= ","
		end
		out ..= string.sub(digits, index, index)
	end
	return out
end

local FORMATS = {
	["image/png"] = "PNG",
	["image/jpeg"] = "JPEG",
	["image/gif"] = "GIF",
	["image/webp"] = "WebP",
	["image/bmp"] = "BMP",
	["image/svg+xml"] = "SVG",
}

-- What `mime` is called: `PNG`, `SVG`.
function canvas.format_name(mime)
	return FORMATS[mime] or mime
end

-- `1200×800 PNG`, or `SVG`.
function canvas.describe(image)
	local format = canvas.format_name(image.mime)
	if image.width ~= nil and image.height ~= nil then
		return image.width .. "×" .. image.height .. " " .. format
	end
	return format
end

-- The revision the pane shows: the one picked, while nothing newer came,
-- or the latest of the item revised last. Its item and place, from 1.
function canvas.shown(of, pick)
	if pick ~= nil and pick.fresh == of.fresh then
		local item = canvas.item(of, pick.item)
		if item ~= nil then
			return item, math.max(1, math.min(pick.revision, #item.revisions))
		end
	end
	local item = of.last and canvas.item(of, of.last)
	if item == nil then
		return nil
	end
	return item, #item.revisions
end

-- A document's first lines, plainly: without the marks that make
-- headings and code; `count` of them at most.
function canvas.excerpt(text, count)
	local lines = {}
	for line in string.gmatch(text .. "\n", "([^\n]*)\n") do
		line = string.gsub(line, "^#+", "")
		line = string.gsub((string.gsub(line, "^%s+", "")), "%s+$", "")
		line = string.gsub(line, "`", "")
		if line ~= "" then
			table.insert(lines, line)
			if #lines == count then
				break
			end
		end
	end
	return lines
end

return canvas
