-- The notes, as their pages show them (heiko-memory's `ui::page`): a
-- search, a link for each kind of note, and the notes in ruled rows. A note
-- opens in place of the list, with a small map of its links and the links
-- themselves under it. What a run recalled and saved, in its transcript.
--
-- A notebook is a scope's current notes, newest first, each as the page
-- shows it: `{ id, title, kind, body, links, paths, written_by, edited }`,
-- `body` its paragraphs (a stale mark, the description, then the body's),
-- `links` `{ to, why }` each, `paths` the files it is about.
local heiko = require("heiko")
local note = require("note")
local ui = heiko.ui

local M = {}

-- What an id of the person's notes starts with.
local USER = "user:"

-- How long before `now` the time `then` was, both in milliseconds, in
-- the largest whole unit: `just now`, `5 minutes ago`, `2 days ago`.
function M.ago(now, then_)
	local seconds = math.floor(math.max(0, now - then_) / 1000)
	local count, unit
	if seconds < 60 then
		return "just now"
	elseif seconds < 3600 then
		count, unit = seconds // 60, "minute"
	elseif seconds < 86400 then
		count, unit = seconds // 3600, "hour"
	elseif seconds < 2592000 then
		count, unit = seconds // 86400, "day"
	elseif seconds < 31536000 then
		count, unit = seconds // 2592000, "month"
	else
		count, unit = seconds // 31536000, "year"
	end
	return heiko.count.plural(count, unit) .. " ago"
end

-- Who wrote a note, in words.
local function by_of(source)
	local imported = source.imported
	if imported then
		return imported.from == "codex" and "from Codex" or "from Claude Code"
	end
	if source.by == "user" then
		return "from the user"
	elseif source.by == "agent" then
		return "by the agent"
	end
	return "inferred"
end

-- One note as the page shows it, edited as long before `now` as it was.
function M.entry(n, now)
	local body = {}
	if n.stale ~= nil then
		table.insert(body, "May be stale: " .. n.stale .. ".")
	end
	if n.description ~= "" then
		table.insert(body, n.description)
	end
	local rest = n.body
	local at = 1
	while true do
		local from, to = string.find(rest, "\n\n", at, true)
		local paragraph = string.sub(rest, at, (from or #rest + 1) - 1)
		paragraph = string.gsub(heiko.text.trim(paragraph), "\n", " ")
		if paragraph ~= "" then
			table.insert(body, paragraph)
		end
		if from == nil then
			break
		end
		at = to + 1
	end
	local seen, paths = {}, {}
	local function path(p)
		if not seen[p] then
			seen[p] = true
			table.insert(paths, p)
		end
	end
	for _, link in ipairs(n.links or {}) do
		if link.type == "about" then
			path(link.to)
		end
	end
	for _, file in ipairs(n.source.files or {}) do
		path(file)
	end
	table.sort(paths, require("import_read").bytes_less)
	local links = {}
	for _, link in ipairs(note.all_links(n)) do
		if link.type ~= "about" then
			table.insert(links, { to = link.to, why = link.why or (string.gsub(link.type, "_", " ")) })
		end
	end
	return {
		id = n.id,
		title = n.title,
		kind = n.type,
		body = body,
		links = links,
		paths = paths,
		written_by = n.type .. " " .. by_of(n.source),
		edited = M.ago(now, n.updated),
	}
end

-- The current notes in `notes` (a scope's store), newest first, as their
-- page lists them. Superseded notes stay in their history, off the list.
function M.notebook(notes, now)
	local current = {}
	for _, n in ipairs(notes:list()) do
		if not note.is_superseded(n) then
			table.insert(current, n)
		end
	end
	table.sort(current, function(a, b)
		if a.updated ~= b.updated then
			return a.updated > b.updated
		end
		return require("import_read").bytes_less(a.id, b.id)
	end)
	local book = {}
	for _, n in ipairs(current) do
		table.insert(book, M.entry(n, now))
	end
	return book
end

local function find(book, id)
	for _, n in ipairs(book) do
		if n.id == id then
			return n
		end
	end
	return nil
end

-- Notes that link to `id`, with why.
local function backlinks(book, id)
	local out = {}
	for _, n in ipairs(book) do
		for _, link in ipairs(n.links) do
			if link.to == id then
				table.insert(out, { note = n, why = link.why })
				break
			end
		end
	end
	return out
end

-- The first sentence, for lists.
local function snippet(n)
	local first = n.body[1] or ""
	local stop = string.find(first, ". ", 1, true)
	return stop and string.sub(first, 1, stop + 1) or first
end

-- A note's page: the scope's, of the page it is drawn on.
local function note_link(label, id)
	return ui.link(label, "notes", { params = { note = id } })
end

-- The list, showing only notes of `kind`; every note without one.
local function kind_link(label, kind)
	return ui.link(label, "notes", { params = kind and { kind = kind } or {} })
end

local PLURALS = {
	fact = "Facts",
	convention = "Conventions",
	decision = "Decisions",
	gotcha = "Gotchas",
	procedure = "Procedures",
	case = "Cases",
	preference = "Preferences",
	index = "Indexes",
}

-- The tone a kind's name is written in.
function M.kind_tone(kind)
	if kind == "fact" or kind == "decision" or kind == "preference" then
		return "info"
	elseif kind == "gotcha" then
		return "warn"
	elseif kind == "procedure" then
		return "good"
	end
	return "neutral"
end

-- Whether `n` matches `query`, already lowercase.
local function matches(n, query)
	if query == "" or string.find(string.lower(n.title), query, 1, true) then
		return true
	end
	for _, paragraph in ipairs(n.body) do
		if string.find(string.lower(paragraph), query, 1, true) then
			return true
		end
	end
	return false
end

local function links_of(n)
	if n == 0 then
		return "no links"
	end
	return heiko.count.plural(n, "link")
end

-- A note in the list: its title on a slip that opens it, then its kind,
-- how linked it is and when it was edited; under them, the start of its
-- text.
local function row(book, n)
	local linked = #n.links + #backlinks(book, n.id)
	return ui.stack({
		ui.row({
			ui.slip(n.title, "teal", false, { page = "notes", params = { note = n.id } }),
			ui.spacer(),
			ui.toned(n.kind, M.kind_tone(n.kind), true),
			ui.toned(links_of(linked), "neutral", true),
			ui.toned(n.edited, "neutral", true),
		}),
		ui.toned((string.gsub(snippet(n), "`", "")), "neutral"),
	})
end

-- The list: `top`, the pieces above it (a banner, or what the person's
-- notes are); the search, the kinds and `button`, the way to import; then
-- a ruled row for each note that matches both `query` and `kind`.
function M.list_tree(book, top, query, kind, button)
	query = string.lower(query or "")
	-- The page's head, unless `top` brings one: its title, and how many
	-- notes there are.
	local headed = false
	for _, piece in ipairs(top) do
		headed = headed or piece.piece == "header"
	end
	local children = {}
	if not headed then
		table.insert(children, ui.header("Memory", { meta = heiko.count.plural(#book, "note") }))
	end
	for _, piece in ipairs(top) do
		table.insert(children, piece)
	end
	local bar = {
		ui.input(M.SEARCH, { placeholder = "Search notes", keep = true }),
		ui.spacer(),
	}
	local function kind_piece(each, name, count)
		local label = name .. " " .. count
		if each == kind then
			table.insert(bar, ui.pill(label, "info"))
		else
			table.insert(bar, kind_link(label, each))
		end
	end
	kind_piece(nil, "All", #book)
	for _, each in ipairs(note.TYPES) do
		local count = 0
		for _, n in ipairs(book) do
			if n.kind == each then
				count += 1
			end
		end
		if count > 0 then
			kind_piece(each, PLURALS[each], count)
		end
	end
	if button then
		table.insert(bar, button)
	end
	table.insert(children, ui.row(bar))
	local shown = 0
	for _, n in ipairs(book) do
		if (kind == nil or n.kind == kind) and matches(n, query) then
			if shown > 0 then
				table.insert(children, ui.rule())
			end
			table.insert(children, row(book, n))
			shown += 1
		end
	end
	if shown == 0 then
		table.insert(children, ui.empty(#book == 0 and "No notes yet." or "No notes match."))
	end
	return ui.stack(children)
end

-- What the person's notes are, over their list: the page's head.
function M.yours()
	return ui.header("Your notes")
end

-- A row for each of `links`: the note, opening it, and why.
local function link_rows(links)
	local out = {}
	for _, link in ipairs(links) do
		table.insert(out, ui.row({ note_link(link.note.title, link.note.id), ui.toned("why: " .. link.why, "neutral") }))
	end
	return out
end

-- Fixed spots, staggered so labels never share a line: links out above
-- the note, backlinks below it.
local ABOVE = { { 0.22, 0.2 }, { 0.78, 0.26 }, { 0.5, 0.06 } }
local BELOW = { { 0.22, 0.7 }, { 0.5, 0.82 }, { 0.78, 0.66 } }

-- The note's links out and in, as a small map and as lists.
local function neighborhood(book, n)
	local out = {}
	for _, link in ipairs(n.links) do
		local to = find(book, link.to)
		if to then
			table.insert(out, { note = to, why = link.why })
		end
	end
	local back = backlinks(book, n.id)
	local nodes = { { id = "note", x = 0.5, y = 0.5, tone = "neutral", focus = true } }
	local edges = {}
	local spots = {}
	for i, link in ipairs(out) do
		if ABOVE[i] then
			table.insert(spots, { "out", link.note, ABOVE[i], "info" })
		end
	end
	for i, link in ipairs(back) do
		if BELOW[i] then
			table.insert(spots, { "back", link.note, BELOW[i], "neutral" })
		end
	end
	for i, spot in ipairs(spots) do
		local id = spot[1] .. "-" .. (i - 1)
		table.insert(edges, { "note", id })
		table.insert(nodes, {
			id = id,
			label = spot[2].title,
			x = spot[3][1],
			y = spot[3][2],
			tone = spot[4],
			page = "notes",
			params = { note = spot[2].id },
		})
	end
	local lists = {
		ui.heading("Neighborhood"),
		ui.graph(nodes, edges, 12.5),
		ui.heading("Links out · " .. #out),
	}
	for _, piece in ipairs(link_rows(out)) do
		table.insert(lists, piece)
	end
	table.insert(lists, ui.heading("Backlinks · " .. #back))
	for _, piece in ipairs(link_rows(back)) do
		table.insert(lists, piece)
	end
	return lists
end

-- A note: the way back, who wrote it and when, its title and text, the
-- files it is about, and its links out and in.
function M.note_tree(book, n)
	local children = {
		kind_link("all notes", nil),
		ui.mono(n.id .. " · " .. n.written_by .. " · edited " .. n.edited),
		ui.title(n.title),
		ui.markdown(table.concat(n.body, "\n\n")),
	}
	if #n.paths > 0 then
		local paths = {}
		for _, p in ipairs(n.paths) do
			table.insert(paths, ui.stamp(p, "neutral"))
		end
		table.insert(children, ui.row(paths))
	end
	for _, piece in ipairs(neighborhood(book, n)) do
		table.insert(children, piece)
	end
	return ui.stack(children)
end

-- The page of `book`: the note `params.note` open, or the list.
function M.page(book, params, top, button)
	local open = params.note and find(book, params.note)
	if open then
		return M.note_tree(book, open)
	end
	local kind = params.kind
	if kind ~= nil and not note.is_type(kind) then
		kind = nil
	end
	local query = (params.drafts or {})[M.SEARCH] or ""
	return M.list_tree(book, top, query, kind, button)
end

-- The notes page's search input.
M.SEARCH = "search"

-- The plugin's name, small and muted, at the end of a mark's first line.
local function name()
	return ui.toned("heiko-memory", "neutral", true)
end

-- A note's title on a slip that opens its page: the page of `repo`'s
-- notes, or for an id of the person's (`user:<id>`), of theirs, as the
-- page with no repository is. Without an id, or a repository for one of
-- its notes, the slip opens nothing.
local function note_slip(title, id, dashed, repo)
	local to
	if id ~= nil and string.sub(id, 1, #USER) == USER then
		to = { page = "notes", params = { note = string.sub(id, #USER + 1) } }
	elseif id ~= nil and repo ~= nil and repo ~= "" then
		to = { page = "notes", params = { note = id, repo = repo } }
	end
	return ui.slip(title, "teal", dashed, to)
end

-- One of memory's marks in a transcript, of a log entry of a run in the
-- repository `repo`: the notes it started with, each on a slip; what it
-- saved, on dashed ones, each opening its note; what failed. Nil for
-- what shows none (a stale mark).
function M.mark(entry, repo)
	local title
	local slips = {}
	if entry.kind == "recalled" then
		title = "Recalled " .. heiko.count.plural(#entry.notes, "note")
		for _, recalled in ipairs(entry.notes) do
			table.insert(slips, note_slip(recalled.title, recalled.id, false, repo))
		end
	elseif entry.kind == "saved" then
		title = "Saved " .. heiko.count.plural(#entry.calls, "note")
		for _, call in ipairs(entry.calls) do
			local saved = call.details
			if saved and (saved.title or saved.id) then
				table.insert(slips, note_slip(saved.title or saved.id, saved.id, true, repo))
			end
		end
	elseif entry.kind == "error" then
		return ui.row({ ui.mark("failing"), ui.text(entry.message), ui.spacer(), name() })
	else
		return nil
	end
	local body = { ui.row({ ui.heading(title), ui.spacer(), name() }) }
	if #slips > 0 then
		table.insert(body, ui.stack(slips))
	end
	return ui.row({ ui.kanji("憶", "teal"), ui.stack(body) })
end

-- The marks of a run's `log`, by key: `m0`, `m1`, … for those that show.
function M.marks(log, repo)
	local out = {}
	local n = 0
	for _, entry in ipairs(log or {}) do
		local piece = M.mark(entry, repo)
		if piece then
			out["m" .. n] = piece
			n += 1
		end
	end
	return out
end

-- The plugin's line in the run's plugin list, of how many notes the
-- repository had as the run started.
function M.status(notes)
	if notes == nil then
		return nil
	end
	if notes == 0 then
		return "no notes yet"
	end
	return heiko.count.plural(notes, "note")
end

return M
