-- What memory draws: the notes pages, a repository's and the person's,
-- the note open with its links, what a run recalled and saved in its
-- transcript and its line in the plugin list, the review page of what
-- imports propose, the banner and menu entry that lead to it, the
-- Memories step of first-run setup, and the settings pane.
local tau = require("tau")
local t = tau.test
local g = t.gen
local memory = require("memory")
local note = require("note")
local read = require("import_read")
local ui_notes = require("ui_notes")
local ui_settings = require("ui_settings")

local HOME = "/home/p"
local REPO_NOTES = "/tau/repos/shop/tau-memory"
local USER_NOTES = "/tau/data/tau-memory"
local SHOP = HOME .. "/work/shop"

local ROOTS = {
	[memory.REPOSITORY] = REPO_NOTES,
	[memory.DATA] = USER_NOTES,
	["~/.claude"] = HOME .. "/.claude",
	["~/.codex"] = HOME .. "/.codex",
}

local function run_with(spec)
	spec = spec or {}
	spec.roots = spec.roots or ROOTS
	spec.repo = spec.repo or "shop"
	spec.repos = spec.repos or { { id = "id-shop", name = "shop", folder = SHOP } }
	spec.repositories = spec.repositories or { shop = "/tau/repos/shop" }
	spec.files = spec.files or {}
	spec.files[SHOP .. "/.keep"] = ""
	return t.run(spec)
end

-- Every piece in `piece`, itself first, depth first.
local function pieces(piece, out)
	out = out or {}
	if type(piece) ~= "table" then
		return out
	end
	if piece.piece ~= nil then
		table.insert(out, piece)
	end
	for key, value in pairs(piece) do
		if type(value) == "table" and key ~= "args" and key ~= "to" then
			pieces(value, out)
		end
	end
	return out
end

-- The texts `piece` shows, of every kind.
local function texts(piece)
	local out = {}
	for _, each in ipairs(pieces(piece)) do
		for _, key in ipairs({ "text", "label", "detail", "subtitle" }) do
			if type(each[key]) == "string" then
				table.insert(out, each[key])
			end
		end
	end
	return out
end

local function shows(piece, text)
	for _, each in ipairs(texts(piece)) do
		if string.find(each, text, 1, true) then
			return true
		end
	end
	return false
end

-- The first piece of kind `kind` in `piece` whose label or text is
-- `label`, when one is given.
local function find(piece, kind, label)
	for _, each in ipairs(pieces(piece)) do
		if each.piece == kind and (label == nil or each.label == label or each.text == label) then
			return each
		end
	end
	return nil
end

local function write(run, args)
	return run:tool("memory_write", args)
end

local function fact(title, extra)
	local args = { type = "fact", title = title, description = "about " .. title, body = "The " .. title .. " holds." }
	for key, value in pairs(extra or {}) do
		args[key] = value
	end
	return args
end

-- A note Claude Code saved for `folder`.
local function claude_note(files, folder, name, kind, body)
	local projects = HOME .. "/.claude/projects/" .. read.encode(folder)
	files[projects .. "/memory/" .. name .. ".md"] = "---\nname: "
		.. name
		.. "\ndescription: "
		.. name
		.. ", in short\nmetadata:\n  type: "
		.. kind
		.. "\n---\n\n"
		.. body
		.. "\n"
end

-- The import's job, ticked until no import is going.
local function settle(run)
	for _ = 1, 200 do
		run:job("import")
		local ok, text = pcall(function()
			return run.files[USER_NOTES .. "/import/checkpoint.json"]
		end)
		local checkpoint = ok and text and json.decode(text) or {}
		if checkpoint.running == nil or checkpoint.running == json.null then
			return
		end
	end
	error("the import never ended")
end

local TITLES = {
	"lanes drain in order",
	"the cache is keyed by model",
	"retries back off",
	"forks start from a turn",
	"rules are checked with jev",
}
local PATHS = { "src/a.rs", "src/b.rs", "docs/c.md" }

-- The screen lists the current notes, newest first, each with its paths
-- (what it is about and came from), its links to other notes, its
-- paragraphs, and a stale mark first when it has one.
t.property("the screen shows current notes as they are", function(draw)
	local run = run_with()
	run:call(function(ctx)
		local scope = memory.open(ctx.files, REPO_NOTES)
		local clock = 1000
		for _ = 1, draw(g.integers({ min = 1, max = 8 })) do
			clock += 1
			local title = draw(g.sampled_from(TITLES))
			local about = {}
			for _, p in ipairs(PATHS) do
				if draw(g.booleans()) then
					table.insert(about, p)
				end
			end
			local links = {}
			for _, p in ipairs(about) do
				table.insert(links, { to = p, type = "about" })
			end
			if draw(g.booleans()) then
				table.insert(links, { to = note.slug(draw(g.sampled_from(TITLES))), type = "refines", why = draw(g.sampled_from({ "", "it says more" })) ~= "" and "it says more" or nil })
			end
			local files = {}
			if draw(g.booleans()) then
				table.insert(files, draw(g.sampled_from(PATHS)))
			end
			local paragraphs = {}
			for _ = 1, draw(g.integers({ min = 1, max = 3 })) do
				table.insert(paragraphs, draw(g.sampled_from({ "One line.", "Two\nlines.", "See [[" .. note.slug(TITLES[1]) .. "]]." })))
			end
			local draft = {
				type = draw(g.sampled_from({ "fact", "gotcha", "decision" })),
				title = title,
				description = "about " .. title,
				body = table.concat(paragraphs, "\n\n"),
				tags = {},
				links = links,
				source = { by = "agent", files = files },
			}
			local existing = scope.notes:ids()
			if #existing > 0 and draw(g.booleans()) then
				draft.supersedes = draw(g.sampled_from(existing))
			end
			-- A title taken without a target is refused; that is fine here.
			scope:write(draft, clock)
			if draw(g.booleans()) then
				clock += 1
				scope:mark_stale({ draw(g.sampled_from(PATHS)) }, "it changed", clock, clock)
			end
		end
		local book = ui_notes.notebook(scope.notes, clock)
		local current = {}
		for _, n in ipairs(scope.notes:list()) do
			if not note.is_superseded(n) then
				table.insert(current, n)
			end
		end
		table.sort(current, function(a, b)
			if a.updated ~= b.updated then
				return a.updated > b.updated
			end
			return read.bytes_less(a.id, b.id)
		end)
		t.equal(#book, #current)
		for i, entry in ipairs(book) do
			local n = current[i]
			t.equal(entry.id, n.id)
			local paths, seen = {}, {}
			for _, link in ipairs(n.links) do
				if link.type == "about" and not seen[link.to] then
					seen[link.to] = true
					table.insert(paths, link.to)
				end
			end
			for _, file in ipairs(n.source.files) do
				if not seen[file] then
					seen[file] = true
					table.insert(paths, file)
				end
			end
			table.sort(paths, read.bytes_less)
			t.equal(entry.paths, paths)
			-- Links to notes, listed or bare in the body, with why, or their
			-- type when no reason was given; paths are not links.
			local links = {}
			for _, link in ipairs(note.all_links(n)) do
				if link.type ~= "about" then
					table.insert(links, { to = link.to, why = link.why or (string.gsub(link.type, "_", " ")) })
				end
			end
			t.equal(entry.links, links)
			for _, link in ipairs(entry.links) do
				t.equal(not table.find(PATHS, link.to), true, "a path is no link")
			end
			t.equal(string.sub(entry.body[1] or "", 1, #"May be stale") == "May be stale", n.stale ~= nil)
			-- The stale mark, then the description, then the body's
			-- paragraphs, one line each.
			local count = 0
			for _ in string.gmatch(n.body .. "\n\n", "(.-)\n\n") do
				count += 1
			end
			t.equal(#entry.body, (n.stale and 1 or 0) + 1 + count)
			for _, p in ipairs(entry.body) do
				t.equal(not string.find(p, "\n", 1, true), true, p)
			end
		end
	end)
end)

-- `ago` names the largest whole unit the time since fits, and never runs
-- backwards as more time passes.
t.property("ago names the largest whole unit", function(draw)
	local then_ = draw(g.integers({ min = 0, max = 2 ^ 40 }))
	local a = draw(g.integers({ min = 0, max = 2 ^ 36 }))
	local b = draw(g.integers({ min = 0, max = 2 ^ 36 }))
	local short, long = math.min(a, b), math.max(a, b)
	-- Each unit, in seconds, and the next unit's size: the count stays
	-- below it, but for years.
	local UNITS = {
		{ "minute", 60, 60 },
		{ "hour", 3600, 24 },
		{ "day", 86400, 30 },
		{ "month", 2592000, 13 },
		{ "year", 31536000, math.huge },
	}
	local function rank(elapsed_ms)
		local said = ui_notes.ago(then_ + elapsed_ms, then_)
		local elapsed = elapsed_ms // 1000
		if said == "just now" then
			t.equal(elapsed < 60, true, said)
			return 0, 0
		end
		local count, word = string.match(said, "^(%d+) (%a+) ago$")
		t.equal(count ~= nil, true, said)
		count = tonumber(count)
		local unit = string.gsub(word, "s$", "")
		local at = nil
		for i, each in ipairs(UNITS) do
			if each[1] == unit then
				at = i
			end
		end
		t.equal(at ~= nil, true, said)
		local seconds, next = UNITS[at][2], UNITS[at][3]
		t.equal(count * seconds <= elapsed and elapsed < (count + 1) * seconds, true, said)
		t.equal(count >= 1 and count < next, true, said)
		t.equal(string.sub(word, -1) == "s", count ~= 1, said)
		return at, count
	end
	local r1, c1 = rank(short)
	local r2, c2 = rank(long)
	t.equal(r1 < r2 or (r1 == r2 and c1 <= c2), true, "it runs backwards")
	t.equal(ui_notes.ago(then_, then_ + long), "just now", "a future time is now")
end)

t.case("ago turns at each unit's start", function()
	local cases = {
		{ 59999, "just now" },
		{ 60000, "1 minute ago" },
		{ 3599999, "59 minutes ago" },
		{ 3600000, "1 hour ago" },
		{ 86399999, "23 hours ago" },
		{ 86400000, "1 day ago" },
		{ 2592000000, "1 month ago" },
		{ 31535999999, "12 months ago" },
		{ 31536000000, "1 year ago" },
		{ 63072000000, "2 years ago" },
	}
	for _, each in ipairs(cases) do
		t.equal(ui_notes.ago(5 + each[1], 5), each[2], tostring(each[1]))
	end
end)

-- What memory's view makes of its log: a note for each recall, save and
-- failure, in order, a stale mark none; and its line from the notes the
-- repository had as the run started.
t.property("the view notes what memory did", function(draw)
	local kinds = draw(g.lists(g.integers({ min = 0, max = 3 }), { max = 8 }))
	local log, expected = {}, {}
	for n, kind in ipairs(kinds) do
		if kind == 0 then
			table.insert(log, { kind = "marked", notes = { "a" }, why = "it changed" })
		elseif kind == 1 then
			table.insert(log, { kind = "recalled", notes = { { id = "n-1", title = "a" } } })
			table.insert(expected, "recalled 1 note for this task")
		elseif kind == 2 then
			local calls = {}
			for _ = 1, n do
				table.insert(calls, { tool = "memory_write" })
			end
			table.insert(log, { kind = "saved", calls = calls })
			table.insert(expected, n == 1 and "saved 1 note" or ("saved " .. n .. " notes"))
		else
			table.insert(log, { kind = "error", message = "offline" })
			table.insert(expected, "offline")
		end
	end
	local notes = draw(g.optional(g.integers({ min = 0, max = 3 })))
	local run = run_with({ state = { log = log, notes = notes } })
	local view = run:view()
	local said = {}
	for i = 0, #expected - 1 do
		local mark = view.notes["m" .. i]
		t.equal(mark ~= nil, true, "mark m" .. i)
		table.insert(said, find(mark, "status").detail)
	end
	t.equal(view.notes["m" .. #expected], nil)
	t.equal(said, expected)
	t.equal(view.status ~= nil, notes ~= nil)
	if notes ~= nil then
		local want = notes == 0 and "no notes yet" or notes == 1 and "1 note" or (notes .. " notes")
		t.equal(view.status.text, want)
	end
end)

t.case("a recalled note links to its page: the repository's, or the person's", function()
	local run = run_with({
		state = {
			log = { { kind = "recalled", notes = { { id = "retry", title = "Retries" }, { id = "user:terse", title = "Terse" } } } },
		},
	})
	local mark = run:view().notes.m0
	local here = find(mark, "link", "Retries")
	t.equal(here.to.params, { repo = "shop", note = "retry" })
	local yours = find(mark, "link", "Terse")
	t.equal(yours.to.params, { note = "terse" })
end)

t.case("a run starts with how many notes its repository has", function()
	local run = run_with()
	write(run, fact("one"))
	write(run, fact("two"))
	run:start({ input = "nothing", model = "m" })
	t.equal(run:view().status.text, "2 notes")
end)

t.case("a repository's notes page lists, filters, searches and opens its notes", function()
	local run = run_with()
	write(run, fact("lanes drain", { links = { { to = "src/lane.rs", type = "about" } } }))
	write(run, fact("cache keys", { type = "gotcha", links = { { to = "lanes-drain", type = "refines", why = "it says when" } } }))
	local page = run:page("notes", { repo = "shop" })
	local list = find(page, "table")
	t.equal(list.head, { "Note", "Kind", "Links", "Edited" })
	t.equal(#list.rows, 2)
	t.equal(find(page, "link", "Facts 1") ~= nil, true, "a link to the facts")
	t.equal(find(page, "link", "Gotchas 1") ~= nil, true, "a link to the gotchas")
	t.equal(find(page, "pill").text, "All 2")
	t.equal(find(page, "input") ~= nil, true, "the search")
	-- No tool on this computer: no way to import.
	t.equal(find(page, "button", "Import from Claude Code or Codex…"), nil)

	-- A link out and a backlink both count.
	for _, r in ipairs(list.rows) do
		t.equal(r[3].text, "1 link")
	end

	local gotchas = run:page("notes", { repo = "shop", kind = "gotcha" })
	t.equal(#find(gotchas, "table").rows, 1)
	t.equal(find(gotchas, "pill").text, "Gotchas 1")

	local searched = run:page("notes", { repo = "shop", drafts = { search = "LANES" } })
	t.equal(#find(searched, "table").rows, 1)
	t.equal(shows(searched, "lanes drain"), true, "the note that matches")
	local none = run:page("notes", { repo = "shop", drafts = { search = "nothing like it" } })
	t.equal(find(none, "empty").text, "No notes match.")

	local open = run:page("notes", { repo = "shop", note = "lanes-drain" })
	t.equal(find(open, "title").text, "lanes drain")
	t.equal(shows(open, "lanes-drain · fact by the agent · edited just now"), true, "who wrote it, and when")
	t.equal(find(open, "badge").text, "src/lane.rs")
	t.equal(shows(open, "Backlinks · 1"), true, "its backlink")
	t.equal(shows(open, "why: it says when"), true, "why it links")
	local graph = find(open, "graph")
	t.equal(#graph.nodes, 2)
	t.equal(graph.nodes[2].to.params, { note = "cache-keys" })

	local empty = run_with():page("notes", { repo = "shop" })
	t.equal(find(empty, "empty").text, "No notes yet.")
end)

t.case("the person's notes are a page of the app, and its sidebar entry counts them", function()
	local run = run_with()
	write(run, fact("terse answers", { type = "preference", scope = "user" }))
	local page = run:app_page_of("notes", {})
	t.equal(find(page, "title").text, "Your notes")
	t.equal(#find(page, "table").rows, 1)
	t.equal(run:app().nav.detail, "1 notes")
	t.equal(run:repo_data().nav.detail, "0 notes")
end)

t.case("an imported note says where it came from", function()
	local files = {}
	claude_note(files, SHOP, "make-check", "project", "Run `make check`.")
	local run = run_with({ files = files })
	run:job("import")
	run:action("import", { request = { parts = { claude_saved = true, claude_chats = false, codex_saved = false, codex_chats = false } } })
	settle(run)
	local review = run:app_page_of("import", {})
	local ids = {}
	for _, each in ipairs(pieces(review)) do
		if each.piece == "switch" and each.args.act and each.args.act.act == "tick" then
			table.insert(ids, each.args.act.id)
		end
	end
	t.equal(#ids, 1)
	run:action("act", { act = { act = "keep", ids = ids } })
	local page = run:page("notes", { repo = "shop", note = "make-check" })
	t.equal(shows(page, "fact from Claude Code"), true, "from Claude Code")
	t.equal(shows(run:app_page_of("import", {}), "Kept 1 note."), true, "what keeping did")
end)

t.case("the review page shows what waits, by tab, and keeps what is ticked", function()
	local files = {}
	claude_note(files, SHOP, "make-check", "project", "Run `make check`.")
	claude_note(files, SHOP, "terse", "feedback", "The person wants terse replies.")
	local run = run_with({ files = files })
	local before = run:app_page_of("import", {})
	t.equal(find(before, "title").text, "No notes to keep")
	run:job("import")
	t.equal(shows(run:app_page_of("import", {}), "Nothing waits. Import to read what Claude Code and Codex kept."), true, "nothing yet")
	local started = run:action("import", { request = {} })
	t.equal(started.tau_interface, { action = "open", args = { page = "import" } })
	settle(run)
	local page = run:app_page_of("import", {})
	t.equal(find(page, "title").text, "2 notes to keep")
	t.equal(find(page, "title").subtitle, "From 2 Claude Code notes.")
	t.equal(find(page, "pill").text, "All 2")
	t.equal(find(page, "link", "About you 1") ~= nil, true, "the person's tab")
	t.equal(find(page, "link", "shop 1") ~= nil, true, "the repository's tab")
	t.equal(find(page, "link", "Need a look 0"), nil)
	t.equal(shows(page, "About you · 1 · saved for every repository"), true, "the person's section")
	t.equal(shows(page, "2 to keep · 0 left for you to decide"), true, "both ticked")
	local yours = run:app_page_of("import", { repo = "you" })
	t.equal(find(yours, "pill").text, "About you 1")
	local searched = run:app_page_of("import", { drafts = { ["review-search"] = "TERSE" } })
	t.equal(shows(searched, "1 to keep · 0 left for you to decide"), true, "one matches")

	-- Unticking one keeps the other alone.
	local tick = nil
	for _, each in ipairs(pieces(page)) do
		if each.piece == "switch" and each.label == "make-check" then
			tick = each
		end
	end
	t.equal(tick ~= nil, true, "a switch per proposal")
	run:action(tick.action, tick.args)
	local after = run:app_page_of("import", {})
	t.equal(shows(after, "1 to keep · 1 left for you to decide"), true, "one unticked")
	local keep = find(after, "button", "Keep 1 note")
	t.equal(keep.kind, "primary")
	run:action(keep.action, keep.args)
	local kept = run:app_page_of("import", {})
	t.equal(shows(kept, "Kept 1 note."), true, "what keeping did")
	t.equal(find(kept, "title").text, "1 note to keep")
	-- Keeping none drops what is in view.
	local drop = find(kept, "button", "Keep none")
	run:action(drop.action, drop.args)
	t.equal(find(run:app_page_of("import", {}), "title").text, "No notes to keep")
	-- With nothing ticked, the keep button asks for nothing.
	t.equal(run:action("act", {}), nil)
end)

t.case("a banner on a repository's notes leads to what its tools kept", function()
	local files = {}
	claude_note(files, SHOP, "make-check", "project", "Run `make check`.")
	local run = run_with({ files = files })
	run:job("import")
	local page = run:page("notes", { repo = "shop" })
	t.equal(shows(page, "Claude Code and Codex know this repository"), true, "the banner")
	t.equal(shows(page, "Claude Code kept 1 note for ~/work/shop."), true, "what they kept")
	local bring = find(page, "button", "Bring what they kept")
	t.equal(bring.kind, "primary")
	t.equal(find(page, "button", "Import from Claude Code or Codex…") ~= nil, true, "the way to import")
	t.equal(run:action(bring.action, bring.args).tau_interface.action, "open")
	settle(run)
	local ready = run:page("notes", { repo = "shop" })
	t.equal(shows(ready, "1 note is ready to look at."), true, "ready")
	local review = find(ready, "button", "Review 1 note")
	t.equal(review.action, "tau.open")
	t.equal(review.args, { page = "import", params = { repo = "shop" } })
	t.equal(find(ready, "button", "Read its 1 chat too"), nil, "no chat to read")
	run:action("act", find(ready, "button", "Not now").args)
	t.equal(not shows(run:page("notes", { repo = "shop" }), "know this repository"), true, "not now hides it")
	-- In the repository's menu, the way to import.
	t.equal(find(run:app().menu, "button", "Import from Claude Code or Codex…") ~= nil, true, "the menu's entry")
end)

t.case("a banner offers to read the repository's chats too, once notes are ready", function()
	local files = {}
	claude_note(files, SHOP, "make-check", "project", "Run `make check`.")
	local projects = HOME .. "/.claude/projects/" .. read.encode(SHOP)
	files[projects .. "/s1.jsonl"] = json.encode({
		type = "user",
		cwd = SHOP,
		timestamp = "2026-10-04T10:00:00Z",
		message = { role = "user", content = "Always run make check." },
	}) .. "\n"
	local run = run_with({ files = files })
	run:job("import")
	-- Nothing is ready yet: the banner brings what they kept, chats too.
	local before = run:page("notes", { repo = "shop" })
	t.equal(find(before, "button", "Bring what they kept") ~= nil, true, "the way to bring them")
	t.equal(find(before, "button", "Read its 1 chat too"), nil, "no chats offered before notes are ready")
	run:action("import", { request = { parts = { claude_saved = true, claude_chats = false, codex_saved = false, codex_chats = false } } })
	settle(run)
	local page = run:page("notes", { repo = "shop" })
	t.equal(find(page, "button", "Review 1 note") ~= nil, true, "ready")
	local chats = find(page, "button", "Read its 1 chat too")
	t.equal(chats ~= nil, true, "the chat to read")
	t.equal(chats.args.act.act, "import")
end)

t.case("the setup step shows each tool found and where its folders go", function()
	local none = run_with()
	none:job("import")
	t.equal(none:setup(none:app(), {}), nil, "no step without a tool")

	local files = {}
	claude_note(files, SHOP, "make-check", "project", "Run `make check`.")
	claude_note(files, SHOP, "terse", "user", "The person wants terse replies.")
	claude_note(files, HOME .. "/work/other", "lint", "project", "Lint first.")
	files[HOME .. "/work/other/.keep"] = ""
	local run = run_with({ files = files })
	run:job("import")
	local app = run:app()
	-- A picked repository no folder of the tool's is in shows no row.
	local step = run:setup(app, { { name = "acme/shop", folder = SHOP }, { name = "acme/none", folder = HOME .. "/work/none" } })
	t.equal(#find(step.body, "table").rows, 3)
	t.equal(step.title, "Memories")
	t.equal(step.at, "after_repositories")
	t.equal(step.heading, "Bring what your other agents learned")
	t.equal(string.find(step.detail, "^Claude Code is on this computer%.") ~= nil, true, step.detail)
	local body = step.body
	t.equal(find(body, "heading").text, "Claude Code")
	t.equal(shows(body, "Its memory · 3 notes"), true, "what it saved")
	t.equal(shows(body, "shop · 1 note"), true, "where the shop folder goes")
	t.equal(shows(body, "About you · 1 note"), true, "what is about the person")
	t.equal(shows(body, "1 other folder"), true, "the folder tau does not have")
	t.equal(shows(body, "left out"), true, "left out, unless asked")
	local others = find(body, "switch", "Also bring folders tau does not have yet, to use when you add them")
	t.equal(others.on, false)
	run:action(others.action, others.args)
	local chosen = run:setup(run:app(), { { name = "acme/shop", folder = SHOP } })
	t.equal(shows(chosen.body, "kept for when you add them"), true, "kept for later")
	local go = find(chosen.body, "button", "Read and show me the notes")
	t.equal(go.kind, "primary")
	t.equal(go.args.request.others, true)
	t.equal(go.args.request.only, { { folder = SHOP } })
	t.equal(run:action(go.action, go.args).tau_interface, { action = "setup_done", args = {} })
	local checkpoint = json.decode(run.files[USER_NOTES .. "/import/checkpoint.json"])
	t.equal(checkpoint.running ~= nil and checkpoint.running ~= json.null, true, "the import started")
end)

t.case("the settings pane binds its settings and says what memory runs as", function()
	local models = { { id = "gpt-6-luna", family = "luna", efforts = { "low", "medium" } } }
	local pane = run_with({ models = models }):settings_view({ family = "luna", reasoning = "low", follow_chat = false, after_each_run = true })
	t.equal(find(pane, "toggle").key, "after_each_run")
	t.equal(find(pane, "choice").options, ui_settings.FAMILIES)
	t.equal(find(pane, "mono").text, "Runs as gpt-6-luna · low.")
	local settings = { family = "luna", reasoning = "high", follow_chat = false, after_each_run = true }
	t.equal(ui_settings.runs_as(settings, models), "Runs as gpt-6-luna · auto.")
	settings.reasoning = "auto"
	t.equal(ui_settings.runs_as(settings, models), "Runs as gpt-6-luna · auto.")
	settings.family = "sol"
	t.equal(ui_settings.runs_as(settings, models), "No sol model on the plan: runs as each chat's model.")
	settings.follow_chat = true
	t.equal(ui_settings.runs_as(settings, models), "Runs as each chat's model.")
end)
