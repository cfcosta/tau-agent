-- What an import shows (ADR 0044; tau-memory's `ui::banner`, `ui::review`
-- and `ui::setup`): the ways into an import, the banner on a repository's
-- notes when the tools know it, the review page of what imports propose,
-- and the Memories step of first-run setup. Each draws from `imports`,
-- what `import.imports` says.
--
-- Their buttons and switches carry out the plugin's actions: `act`, with
-- `{ act }` for the import (`import.act`); `import`, which starts one and
-- opens the review page; `setup_choose` and `setup_start`, for the setup
-- step; and tau's `tau.open`, which opens a page.
local tau = require("tau")
local import = require("import")
local ui_notes = require("ui_notes")
local ui = tau.ui

local M = {}

-- What the way into an import says.
M.IMPORT = "Import from Claude Code or Codex…"

-- The review page's search input.
M.SEARCH = "review-search"

local function plural(n, one, many)
	return n .. " " .. (n == 1 and one or many)
end

-- Whether either tool is on this computer.
function M.any_found(imports)
	return #(imports.found or {}) > 0
end

local function act(what)
	return { act = what }
end

-- Starts an import of every listed repository, and opens the review page
-- to watch it.
local function start(label, kind, request)
	return ui.button(label, "import", { request = request or {} }, kind)
end

-- The notes page's button, when either tool is on this computer.
function M.import_button(imports)
	if not M.any_found(imports) then
		return nil
	end
	return start(M.IMPORT, "secondary")
end

-- A repository menu's entry, when either tool is on this computer.
function M.menu(imports)
	if not M.any_found(imports) then
		return nil
	end
	return ui.row({ ui.icon("memory", "neutral"), start(M.IMPORT, "secondary") })
end

-- What the tools kept for the repository, in words.
local function kept(banner)
	local parts, tools = {}, {}
	for _, saved in ipairs(banner.saved) do
		local tool, n = saved[1], saved[2]
		table.insert(parts, tool == "codex" and plural(n, "task summary", "task summaries") or plural(n, "note", "notes"))
		table.insert(tools, import.label(tool))
	end
	if #tools == 1 then
		return tools[1] .. " kept " .. parts[1] .. " for " .. banner.folder .. "."
	elseif #tools == 2 then
		return tools[1] .. " kept " .. parts[1] .. " for " .. banner.folder .. ", and " .. tools[2] .. " " .. parts[2] .. "."
	end
	return "Claude Code or Codex worked in " .. banner.folder .. "."
end

-- The banner on `repo`'s notes, when its tools know it.
function M.banner(repo, imports)
	local banner = (imports.banners or {})[repo]
	if banner == nil then
		return nil
	end
	local text = kept(banner)
	if banner.ready > 0 then
		text ..= " " .. (banner.ready == 1 and "1 note is" or (banner.ready .. " notes are")) .. " ready to look at."
	end
	local buttons = {}
	if banner.ready > 0 then
		table.insert(
			buttons,
			ui.button(
				"Review " .. plural(banner.ready, "note", "notes"),
				"tau.open",
				{ page = "import", params = { repo = repo } },
				"primary"
			)
		)
	else
		table.insert(buttons, start("Bring what they kept", "primary", { only = banner.places }))
	end
	if banner.unread_chats > 0 and banner.ready > 0 then
		-- Reads the chats too, staying on the notes.
		table.insert(
			buttons,
			ui.button(
				"Read its " .. plural(banner.unread_chats, "chat", "chats") .. " too",
				"act",
				act({ act = "import", only = banner.places }),
				"secondary"
			)
		)
	end
	table.insert(buttons, ui.button("Not now", "act", act({ act = "not_now", repo = repo }), "secondary"))
	return ui.card({
		ui.heading("Claude Code and Codex know this repository"),
		ui.text(text),
		ui.row(buttons),
	})
end

-- The review page ----------------------------------------------------------

-- The tab `params` shows: `{ kind = "all" | "you" | "look" }`, or a
-- repository's, `{ kind = "repo", repo }`.
function M.tab_of(params)
	if params.look ~= nil then
		return { kind = "look" }
	end
	local repo = params.repo
	if repo == nil or repo == "" or repo == "all" then
		return { kind = "all" }
	elseif repo == "you" then
		return { kind = "you" }
	end
	return { kind = "repo", repo = repo }
end

-- The repository a proposal waits for, nil for the person's.
local function repo_of(pending)
	if pending.repo == json.null then
		return nil
	end
	return pending.repo
end

local function holds(tab, pending)
	if tab.kind == "all" then
		return true
	elseif tab.kind == "you" then
		return repo_of(pending) == nil
	elseif tab.kind == "repo" then
		return repo_of(pending) == tab.repo
	end
	return import.need_a_look(pending.flags)
end

local function same_tab(a, b)
	return a.kind == b.kind and a.repo == b.repo
end

local function tab_link(tab, label)
	local params = {}
	if tab.kind == "you" then
		params.repo = "you"
	elseif tab.kind == "repo" then
		params.repo = tab.repo
	elseif tab.kind == "look" then
		params.look = "1"
	end
	return ui.link(label, "import", { params = params })
end

local function tab_name(tab)
	if tab.kind == "all" then
		return "All"
	elseif tab.kind == "you" then
		return "About you"
	elseif tab.kind == "repo" then
		return tab.repo
	end
	return "Need a look"
end

-- Where the proposals came from, and what was left out or merged: "From
-- 501 Claude Code notes and 17 Codex summaries: 380 were about folders
-- you left out, 15 said the same as another."
function M.summary(imports)
	local tally = imports.tally or { read = {} }
	local function read_of(tool, material)
		return (tally.read or {})[import.tally_key(tool, material)] or 0
	end
	local from = {}
	local claude = read_of("claude_code", "saved")
	if claude > 0 then
		table.insert(from, claude .. " Claude Code " .. (claude == 1 and "note" or "notes"))
	end
	local codex = read_of("codex", "saved")
	if codex > 0 then
		table.insert(from, codex .. " Codex " .. (codex == 1 and "summary" or "summaries"))
	end
	local chats = read_of("claude_code", "chat") + read_of("codex", "chat")
	if chats > 0 then
		table.insert(from, plural(chats, "chat", "chats"))
	end
	if #from == 0 then
		return nil
	end
	local said
	if #from == 1 then
		said = from[1]
	else
		local last = table.remove(from)
		said = table.concat(from, ", ") .. " and " .. last
	end
	local thens = {}
	local function count(n, one, many, rest)
		if n > 0 then
			table.insert(thens, (n == 1 and one or (n .. " " .. many)) .. " " .. rest)
		end
	end
	count(tally.left_out or 0, "1 was", "were", "about folders you left out")
	count(tally.merged or 0, "1 said", "said", "the same as another")
	count(tally.known or 0, "1 said", "said", "what a note says already")
	count(tally.refused or 0, "1 was", "were", "refused as instructions to the model")
	if #thens == 0 then
		return "From " .. said .. "."
	end
	return "From " .. said .. ": " .. table.concat(thens, ", ") .. "."
end

-- What keeping did.
local function said(kept_)
	local line = "Kept " .. plural(kept_.kept, "note", "notes") .. "."
	for _, refused in ipairs(kept_.refused or {}) do
		line ..= " " .. refused[1] .. " was not kept: " .. refused[2] .. "."
	end
	return line
end

-- How many wait, where they came from, and the import going on.
local function head(imports)
	local n = #imports.pending
	local out = { ui.title(n == 0 and "No notes to keep" or (plural(n, "note", "notes") .. " to keep"), M.summary(imports)) }
	local progress = imports.progress
	if progress ~= nil and progress ~= json.null then
		table.insert(out, ui.progress(progress.fraction, string.format("%s · %.0f%%", progress.line, progress.fraction * 100)))
		table.insert(
			out,
			ui.row({
				ui.text("New notes join this list as they come."),
				ui.button("Stop", "act", act("stop"), "secondary"),
			})
		)
	elseif M.any_found(imports) then
		table.insert(out, ui.row({ ui.spacer(), ui.button(M.IMPORT, "act", act({ act = "import" }), "secondary") }))
	end
	return out
end

-- The tabs, with how many each holds, and the search.
local function tabs(imports, on)
	local function count(tab)
		local n = 0
		for _, pending in ipairs(imports.pending) do
			if holds(tab, pending) then
				n += 1
			end
		end
		return n
	end
	local all = { { kind = "all" }, { kind = "you" } }
	local repos, seen = {}, {}
	for _, pending in ipairs(imports.pending) do
		local repo = repo_of(pending)
		if repo ~= nil and not seen[repo] then
			seen[repo] = true
			table.insert(repos, repo)
		end
	end
	table.sort(repos, require("import_read").bytes_less)
	for _, repo in ipairs(repos) do
		table.insert(all, { kind = "repo", repo = repo })
	end
	table.insert(all, { kind = "look" })
	local row = {}
	for _, tab in ipairs(all) do
		local n = count(tab)
		-- A repository's tab, and Need a look, only with something in.
		if not (n == 0 and (tab.kind == "repo" or tab.kind == "look")) then
			local label = tab_name(tab) .. " " .. n
			if same_tab(tab, on) then
				table.insert(row, ui.pill(label, tab.kind == "look" and "warn" or "info"))
			else
				table.insert(row, tab_link(tab, label))
			end
		end
	end
	table.insert(row, ui.spacer())
	table.insert(row, ui.icon("search", "neutral"))
	table.insert(row, ui.input(M.SEARCH, { placeholder = "Search", keep = true }))
	return ui.row(row)
end

-- One proposal: its switch, title, description and where it came from;
-- its type and flags.
local function proposal_row(pending)
	local flags = { ui.badge(pending.type, ui_notes.kind_tone(pending.type)) }
	if pending.in_both then
		local first = pending.origins[1]
		local other = (first ~= nil and string.sub(first, 1, #"Codex") == "Codex") and "Claude Code" or "Codex"
		table.insert(flags, ui.badge("also in " .. other, "info"))
	end
	local f = pending.flags or {}
	if f.disagrees ~= nil and f.disagrees ~= json.null then
		table.insert(flags, ui.badge("disagrees with " .. f.disagrees, "warn"))
	end
	if f.replaces ~= nil and f.replaces ~= json.null then
		table.insert(flags, ui.badge("replaces " .. f.replaces, "warn"))
	end
	if f.secret == true then
		table.insert(flags, ui.badge("held a secret", "bad"))
	end
	local from = ""
	local origins = pending.origins or {}
	if #origins == 1 then
		from = origins[1]
	elseif #origins > 1 then
		from = origins[1] .. ", and " .. plural(#origins - 1, "more", "more")
	end
	local detail = from == "" and pending.description or (pending.description .. " · " .. from)
	return {
		ui.switch(pending.title, detail, pending.keep, "act", act({ act = "tick", id = pending.id, keep = not pending.keep })),
		ui.row(flags),
	}
end

-- The proposals in view, a heading and a table per scope.
local function list(shown, imports)
	if #shown == 0 then
		local text
		if #imports.pending == 0 then
			local progress = imports.progress
			if progress ~= nil and progress ~= json.null then
				text = "Nothing yet: proposals show here as they come."
			elseif M.any_found(imports) then
				text = "Nothing waits. Import to read what Claude Code and Codex kept."
			else
				text = "Nothing waits, and neither Claude Code nor Codex is on this computer."
			end
		else
			text = "Nothing here."
		end
		return { ui.empty(text) }
	end
	local sections = {}
	for _, pending in ipairs(shown) do
		local found = nil
		for _, section in ipairs(sections) do
			if section.repo == repo_of(pending) then
				found = section
			end
		end
		if found == nil then
			found = { repo = repo_of(pending), rows = {} }
			table.insert(sections, found)
		end
		table.insert(found.rows, pending)
	end
	-- The person's first, then the repositories by name.
	table.sort(sections, function(a, b)
		if (a.repo == nil) ~= (b.repo == nil) then
			return a.repo == nil
		end
		if a.repo == nil then
			return false
		end
		return require("import_read").bytes_less(a.repo, b.repo)
	end)
	local out = {}
	for _, section in ipairs(sections) do
		local heading = (section.repo or "About you") .. " · " .. #section.rows
		if section.repo == nil then
			heading ..= " · saved for every repository"
		end
		local rows = {}
		for _, pending in ipairs(section.rows) do
			table.insert(rows, proposal_row(pending))
		end
		table.insert(out, ui.heading(heading))
		table.insert(out, ui.table({}, rows, { widths = { 5, 2 } }))
	end
	return out
end

-- How many are ticked, and the buttons that keep them or drop all.
local function foot(shown)
	local ticked, all = {}, {}
	for _, pending in ipairs(shown) do
		table.insert(all, pending.id)
		if pending.keep then
			table.insert(ticked, pending.id)
		end
	end
	local label = #ticked == 1 and "Keep 1 note" or ("Keep " .. #ticked .. " notes")
	local row = {
		ui.text(#ticked .. " to keep · " .. (#shown - #ticked) .. " left for you to decide"),
		ui.spacer(),
		ui.button("Keep none", "act", act({ act = "drop", ids = all }), "secondary"),
	}
	if #ticked == 0 then
		-- With nothing ticked the button keeps nothing: it asks for nothing.
		table.insert(row, ui.button(label, "act", {}, "secondary"))
	else
		table.insert(row, ui.button(label, "act", act({ act = "keep", ids = ticked }), "primary"))
	end
	return ui.row(row)
end

-- The files that did not read.
local function failed(imports)
	local children = { ui.heading("Could not read " .. plural(#imports.failed, "file", "files")) }
	for _, path in ipairs(imports.failed) do
		table.insert(children, ui.mono(path))
	end
	return ui.stack(children)
end

-- The review page's tree: the proposals of `tab` that match `query`, and
-- what keeping last did, `kept_`.
function M.review(imports, tab, query, kept_)
	query = string.lower(query or "")
	local shown = {}
	for _, pending in ipairs(imports.pending) do
		if
			holds(tab, pending)
			and (
				query == ""
				or string.find(string.lower(pending.title), query, 1, true)
				or string.find(string.lower(pending.description), query, 1, true)
			)
		then
			table.insert(shown, pending)
		end
	end
	local children = head(imports)
	table.insert(children, tabs(imports, tab))
	if kept_ ~= nil then
		table.insert(children, ui.text(said(kept_)))
	end
	for _, piece in ipairs(list(shown, imports)) do
		table.insert(children, piece)
	end
	if #shown > 0 then
		table.insert(children, foot(shown))
	end
	if #(imports.failed or {}) > 0 then
		table.insert(children, failed(imports))
	end
	return ui.stack(children)
end

-- The setup step --------------------------------------------------------------

-- What the person chose so far, as the setup step keeps it: every part,
-- and not the folders tau does not have.
function M.choice(kept_)
	local parts = {}
	for _, key in ipairs({ "claude_saved", "claude_chats", "codex_saved", "codex_chats" }) do
		parts[key] = not (kept_ and kept_.parts and kept_.parts[key] == false)
	end
	return { parts = parts, others = (kept_ and kept_.others) == true }
end

local function parts_key(tool, material)
	return (tool == "claude_code" and "claude_" or "codex_") .. (material == "saved" and "saved" or "chats")
end

local function with(choice, change)
	local next = { parts = table.clone(choice.parts), others = choice.others }
	change(next)
	return next
end

-- `value`, nil for JSON's null.
local function given(value)
	if value == json.null then
		return nil
	end
	return value
end

-- A place as `import.matches` reads it: its folder and remote, or nil.
local function place_of(place)
	return { folder = given(place.folder), remote = given(place.remote) }
end

-- The repository's short name: `hello` of `octocat/hello`.
local function short(name)
	return string.match(name, "([^/]*)$") or name
end

-- One tool: what it has, what to read of it, and where it goes.
local function card(found, picked, choice)
	local tool = found.tool
	local folders = 0
	for _, folder in ipairs(found.folders) do
		if folder.saved > 0 then
			folders += 1
		end
	end
	local saved, saved_detail, chats, chats_detail
	if tool == "claude_code" then
		saved = "Its memory · " .. plural(found.saved, "note", "notes")
		saved_detail = "Notes it saved in " .. plural(folders, "folder", "folders") .. ". Copied as they are, without the model."
		chats = "Its chats · " .. plural(found.chats, "conversation", "conversations")
		chats_detail = "Your model reads them and writes down what was decided, fixed or preferred."
	else
		saved = "Its memories · " .. plural(found.saved, "summary", "summaries")
		saved_detail = "What Codex kept from past tasks, grouped by folder. Your model splits them into notes."
		chats = "Its chats · " .. plural(found.chats, "session", "sessions")
		chats_detail = "Read in full by your model. It keeps going after setup."
	end
	local function what(material, label, detail)
		local key = parts_key(tool, material)
		local on = choice.parts[key] ~= false
		local next = with(choice, function(next)
			next.parts[key] = not on
		end)
		return ui.switch(label, detail, on, "setup_choose", { choice = next })
	end
	local one, many = "note", "notes"
	if tool == "codex" then
		one, many = "summary", "summaries"
	end
	local rows = {}
	local claimed = {}
	for _, repo in ipairs(picked) do
		local count, label = 0, nil
		for n, folder in ipairs(found.folders) do
			if import.matches(place_of(folder.place), repo.folder, repo.remote) then
				claimed[n] = true
				count += folder.saved
				label = label or folder.label
			end
		end
		if label then
			table.insert(rows, { label, short(repo.name) .. " · " .. plural(count, one, many) })
		end
	end
	if found.about_you > 0 then
		table.insert(rows, { "about you, in any folder", "About you · " .. plural(found.about_you, one, many) })
	end
	local left = 0
	for n in ipairs(found.folders) do
		if not claimed[n] then
			left += 1
		end
	end
	if left > 0 then
		table.insert(rows, {
			plural(left, "other folder", "other folders"),
			choice.others and "kept for when you add them" or "left out",
		})
	end
	local table_rows = {}
	for _, r in ipairs(rows) do
		table.insert(table_rows, { ui.mono(r[1]), ui.text("→"), ui.text(r[2]) })
	end
	return ui.card({
		ui.heading(import.label(tool)),
		ui.mono(tool == "codex" and "~/.codex" or "~/.claude"),
		what("saved", saved, saved_detail),
		what("chat", chats, chats_detail),
		ui.table({}, table_rows, { widths = { 4, 0.5, 4 } }),
	})
end

-- Whether any part of `choice` is read.
local function any(choice)
	for _, on in pairs(choice.parts) do
		if on then
			return true
		end
	end
	return false
end

-- The step's tree: a card for each tool found, whether to bring the
-- folders tau does not have, and the button that starts the import.
function M.setup_tree(found, picked, choice)
	local children = {}
	for _, each in ipairs(found) do
		table.insert(children, card(each, picked, choice))
	end
	table.insert(
		children,
		ui.switch(
			"Also bring folders tau does not have yet, to use when you add them",
			nil,
			choice.others,
			"setup_choose",
			{ choice = with(choice, function(next)
				next.others = not choice.others
			end) }
		)
	)
	local request = nil
	if any(choice) then
		local only = {}
		for _, repo in ipairs(picked) do
			table.insert(only, { folder = repo.folder, remote = repo.remote })
		end
		request = { parts = choice.parts, only = only, others = choice.others }
	end
	table.insert(
		children,
		ui.row({
			ui.text("Only chats in the folders above"),
			ui.spacer(),
			ui.button(
				"Read and show me the notes",
				"setup_start",
				{ request = request },
				request and "primary" or "secondary"
			),
		})
	)
	table.insert(
		children,
		ui.text(
			"Read only: tau never changes Claude Code's or Codex's files. What they wrote is treated as notes to keep, never as instructions."
		)
	)
	return ui.stack(children)
end

-- The step, when either tool is on this computer: `{ step, at, title,
-- heading, detail, body }`.
function M.setup(imports, picked, choice)
	if not M.any_found(imports) then
		return nil
	end
	local names = {}
	for _, found in ipairs(imports.found) do
		table.insert(names, import.label(found.tool))
	end
	local on = #names == 1 and (names[1] .. " is") or (table.concat(names, " and ") .. " are")
	return {
		step = "memories",
		at = "after_repositories",
		title = "Memories",
		heading = "Bring what your other agents learned",
		detail = on
			.. " on this computer. tau can turn what they remember into its own notes, for the repositories you picked and about you. You see every note before it is kept.",
		body = M.setup_tree(imports.found, picked, choice),
	}
end

return M
