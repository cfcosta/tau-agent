-- What tau's demo shows of memory (crates/tau-demo), drawn by the plugin
-- here and kept as the fixture `demo`: the notes tau-agent, docbert and
-- homelab.nix's runs kept, each repository's page and data, a note open,
-- the review page while an import reads chats, docbert's banner, the
-- Memories step of first-run setup, the settings pane, and what the demo
-- run recalled and saved. The imports are synthetic: no one's real notes.
local tau = require("tau")
local t = tau.test
local note = require("note")
local ui_notes = require("ui_notes")
local ui_import = require("ui_import")

-- The demo's now: every note's age is counted from it.
local NOW = 1790000000000
local DAY = 24 * 60 * 60 * 1000

-- A fact the agent kept `days` ago: its first paragraph is its
-- description, and it is about `paths`.
local function fact(id, title, paragraphs, links, paths, days)
	local at = NOW - days * DAY
	local rest = {}
	for i = 2, #paragraphs do
		table.insert(rest, paragraphs[i])
	end
	local source = note.source("agent")
	source.files = paths
	return {
		id = id,
		title = title,
		description = paragraphs[1],
		type = "fact",
		tags = {},
		created = at,
		updated = at,
		valid_from = at,
		source = source,
		links = links,
		body = table.concat(rest, "\n\n"),
	}
end

local function link(to, why)
	return { to = to, type = "relates", why = why }
end

local NOTES = {
	["tau-agent"] = {
		fact(
			"n-0417",
			"Rotation must drain lanes first",
			{
				"A connection that is past its deadline can still carry lanes with a response in flight. Retiring it right away breaks their continuation: the next request would name a `previous_response_id` that the new socket has never seen.",
				"So the pool marks the connection as draining, sends no new lanes to it, and closes it when its last lane finishes.",
				"Jitter on the deadline only moves when draining starts. It does not replace it.",
			},
			{ link("n-0212", "how a lane knows what it continues"), link("n-0433", "jitter moves the deadline") },
			{ "crates/tau-ai/src/ws/proto/pool.rs", "crates/tau-ai/src/ws/proto/lane.rs" },
			7
		),
		fact(
			"n-0433",
			"Spread reconnects with jitter",
			{
				"Connections opened together expire together unless the deadline moves. A jitter drawn at open time spreads the rotations.",
			},
			{ link("n-0417", "draining still applies") },
			{ "crates/tau-ai/src/ws/proto/pool.rs" },
			2
		),
		fact(
			"n-0212",
			"Continuation ids are call_id|item_id",
			{
				"The Responses API needs both halves to resume a tool call. tau-ai joins them with a `|` in the tool call id.",
			},
			{},
			{ "crates/tau-ai/src/responses/input.rs" },
			9
		),
		fact(
			"n-0301",
			"16 in flight per connection",
			{ "The pool enforces the limit per socket, not per client. Draining sockets still count." },
			{ link("n-0417", "draining sockets still count") },
			{ "crates/tau-ai/src/ws/proto/pool.rs" },
			3
		),
		fact(
			"n-0388",
			"Retry policy honors server hints",
			{
				"When the server sends `retry-after`, it wins over our backoff, capped at `max_delay`. The header can be seconds or an HTTP date.",
			},
			{ link("n-0212", "retries resume the same continuation") },
			{ "crates/tau-ai/src/retry.rs" },
			4
		),
		fact(
			"n-0390",
			"429 vs 503 in the Responses API",
			{
				"429 means we sent too much; 503 means they are overloaded. Both are retried, and both may carry `retry-after`.",
			},
			{ link("n-0388", "both carry the hint") },
			{ "crates/tau-ai/src/retry.rs" },
			2
		),
		fact(
			"n-0205",
			"Tests use the fake OpenAI server",
			{ "`tau-testing` runs a fake Responses server over WebSocket. Tests never reach the network." },
			{},
			{ "crates/tau-testing/src/fake_openai.rs" },
			11
		),
		fact(
			"n-0350",
			"Rewrites force one full resend",
			{
				"Any edit to the transcript breaks the delta chain once. The next request goes in full, and turns are deltas again after it.",
			},
			{ link("n-0417", "a resend can land on a draining socket") },
			{ "crates/tau-agent/src/plugin.rs" },
			5
		),
	},
	docbert = {
		fact(
			"d-0102",
			"Scanned pages have no text layer",
			{
				"PDFs made from scans come in with empty pages. Run OCR on a page only when it has no text layer, so born-digital PDFs stay fast.",
			},
			{},
			{ "src/ingest/pdf.rs" },
			3
		),
		fact(
			"d-0118",
			"Rerank only the top 50",
			{
				"ColBERT reranking costs grow with the candidate list. BM25 recalls 200, and only the top 50 go to the reranker.",
			},
			{ link("d-0131", "BM25 recalls the candidates") },
			{ "src/search/rerank.rs" },
			4
		),
		fact(
			"d-0131",
			"The BM25 tokenizer keeps identifiers whole",
			{ "`snake_case` and `CamelCase` stay one token, and are also split into their parts, so both searches hit." },
			{},
			{ "src/search/bm25.rs" },
			2
		),
	},
	["homelab.nix"] = {
		fact(
			"h-0007",
			"Backups run from a systemd timer",
			{ "`restic` runs from `backup.timer` at 03:00, never from cron, so a missed run catches up on boot." },
			{},
			{ "hosts/nas/backup.nix" },
			1
		),
	},
}

-- Where the demo's repositories are, and what their Git remotes say.
local function place(name)
	return { folder = "/home/demo/Code/cfcosta/" .. name, remote = "git@github.com:cfcosta/" .. name .. ".git" }
end

-- What the demo's imports show: Claude Code and Codex on the demo's
-- computer, proposals for tau-agent, docbert and the person, the import
-- reading chats, and a banner on docbert.
local function imports()
	local function folder(name, saved, chats)
		return {
			place = place(name),
			label = "~/Code/cfcosta/" .. name,
			saved = saved,
			chats = chats,
			unread_saved = saved,
			unread_chats = chats,
		}
	end
	local function found(tool, saved, chats, about_you, own, others, others_saved)
		local folders = own
		for i = 0, others - 1 do
			table.insert(folders, {
				place = { folder = "/home/demo/scratch/project-" .. i, remote = json.null },
				label = "~/scratch/project-" .. i,
				saved = others_saved,
				chats = 2,
				unread_saved = others_saved,
				unread_chats = 2,
			})
		end
		return { tool = tool, saved = saved, chats = chats, about_you = about_you, folders = folders }
	end
	local function pending(id, kind, title, description, repo, origins, flags)
		local claude, codex = false, false
		for _, origin in ipairs(origins) do
			claude = claude or string.sub(origin, 1, 11) == "Claude Code"
			codex = codex or string.sub(origin, 1, 5) == "Codex"
		end
		local look = flags.secret or flags.disagrees ~= nil
		return {
			id = id,
			type = kind,
			title = title,
			description = description,
			repo = repo or json.null,
			origins = origins,
			in_both = claude and codex,
			keep = not look,
			flags = { secret = flags.secret or false, disagrees = flags.disagrees },
		}
	end
	return {
		found = {
			found("claude_code", 501, 180, 18, { folder("tau-agent", 41, 64), folder("docbert", 9, 23) }, 52, 8),
			found("codex", 93, 748, 0, { folder("tau-agent", 14, 120), folder("docbert", 4, 30) }, 31, 2),
		},
		pending = {
			pending(
				"p-1",
				"preference",
				"Answers lead with the result",
				"The outcome first, then the detail that backs it.",
				nil,
				{ "Claude Code · memory/result-first.md" },
				{}
			),
			pending(
				"p-2",
				"preference",
				"Commits are small, and each one builds",
				"One change per commit; every commit passes the tests.",
				nil,
				{ "Claude Code · memory/small-commits.md", "Codex · task summary, 2026-10-09" },
				{}
			),
			pending(
				"p-3",
				"procedure",
				"Run the integration suite with --features live",
				"Says the suite needs the staging database; tau has n-0441 saying it runs offline.",
				"tau-agent",
				{ "Codex · task summary, 2026-10-09" },
				{ disagrees = "n-0441" }
			),
			pending(
				"p-4",
				"gotcha",
				"The fixture server needs a free port",
				"Bind port 0 and read it back; a fixed port collides in CI.",
				"tau-agent",
				{ "Claude Code · chat on 2026-10-04" },
				{}
			),
			pending(
				"p-5",
				"fact",
				"[redacted] token for the staging API",
				"The chat pasted a secret; tau kept it out of the note.",
				"tau-agent",
				{ "Codex · chat on 2026-09-22" },
				{ secret = true }
			),
			pending(
				"p-6",
				"fact",
				"Searches go through the BM25 index first",
				"Candidates come from BM25 before ColBERT reranks them.",
				"docbert",
				{ "Claude Code · memory/bm25-first.md" },
				{}
			),
			pending(
				"p-7",
				"decision",
				"Embeddings are rebuilt when the model changes",
				"The cache is keyed by model, so a new model re-encodes.",
				"docbert",
				{ "Codex · task summary, 2026-10-02" },
				{}
			),
		},
		held = 0,
		progress = { fraction = 0.36, line = "Reading Claude Code chats · 64 of 180" },
		tally = {
			read = { ["claude_code.saved"] = 501, ["codex.saved"] = 17 },
			left_out = 380,
			merged = 15,
			known = 0,
			refused = 0,
		},
		failed = { "~/.codex/sessions/2026/09/30/rollout-cut-short.jsonl" },
		banners = {
			docbert = {
				folder = "~/Code/cfcosta/docbert",
				places = { place("docbert") },
				saved = { { "claude_code", 9 }, { "codex", 4 } },
				unread_chats = 23,
				unread_saved = 0,
				ready = 2,
			},
		},
	}
end

-- A repository's notes as a scope's store lists them.
local function book_of(repo)
	local notes = NOTES[repo]
	return ui_notes.notebook({
		list = function()
			return notes
		end,
	}, NOW)
end

-- The plan's models as the demo has them, newest of each family first.
local MODELS = {
	{ id = "gpt-6-luna", family = "luna", efforts = { "low", "medium", "high" } },
	{ id = "gpt-6-astra", family = "astra", efforts = { "low", "medium", "high", "xhigh" } },
	{ id = "gpt-5.5", family = "sol", efforts = { "low", "medium", "high", "xhigh" } },
}

-- What the demo run's transcript holds of memory: the three notes it
-- recalled as it started, then the two it saved as it ended.
local RECALLED = {
	kind = "recalled",
	notes = {
		{ id = "n-0388", title = "Retry policy honors server hints" },
		{ id = "n-0390", title = "429 vs 503 in the Responses API" },
		{ id = "n-0205", title = "Tests use the fake OpenAI server" },
	},
}
local SAVED = {
	kind = "saved",
	calls = { { tool = "memory_write", details = {} }, { tool = "memory_write", details = {} } },
}

t.fixture("demo", function()
	local all = imports()
	local button = ui_import.import_button(all)
	local pages, repo_data = {}, {}
	for repo in pairs(NOTES) do
		local book = book_of(repo)
		local top = {}
		local banner = ui_import.banner(repo, all)
		if banner then
			table.insert(top, banner)
		end
		pages[repo] = ui_notes.page(book, {}, top, button)
		repo_data[repo] = {
			nav = { detail = #book .. " notes" },
			notes = #book,
			edited = book[1] and book[1].edited or nil,
			banner = all.banners[repo],
		}
	end
	pages["tau-agent/n-0417"] = ui_notes.page(book_of("tau-agent"), { note = "n-0417" }, {}, button)
	pages.import = ui_import.review(all, ui_import.tab_of({}), "", nil)
	local app = {
		nav = { detail = "0 notes" },
		you = { notes = 0 },
		imports = all,
		menu = ui_import.menu(all),
	}
	local picked = {
		{ name = "tau-agent", folder = place("tau-agent").folder, remote = place("tau-agent").remote },
		{ name = "docbert", folder = place("docbert").folder, remote = place("docbert").remote },
	}
	local run = function(log)
		local state = { notes = #NOTES["tau-agent"], log = log }
		return { state = state, view = t.run({ repo = "tau-agent", state = state }):view() }
	end
	local settings = t.run({ models = MODELS }):settings_view({
		family = "luna",
		reasoning = "low",
		follow_chat = false,
		after_each_run = true,
	})
	return {
		pages = pages,
		repo_data = repo_data,
		app = app,
		setup = ui_import.setup(all, picked, ui_import.choice(nil)),
		settings = settings,
		run = { run({ RECALLED }), run({ RECALLED, SAVED }) },
	}
end)

-- Every link between the demo's notes lands on a note of the same
-- repository, and the note the demo opens has others leading to it.
t.case("the demo's notes link within their repository", function()
	for repo, notes in pairs(NOTES) do
		local ids = {}
		for _, n in ipairs(notes) do
			t.equal(note.validate(n), nil, n.id)
			ids[n.id] = true
		end
		for _, n in ipairs(notes) do
			for _, l in ipairs(n.links) do
				t.equal(ids[l.to], true, repo .. ": " .. n.id .. " -> " .. l.to)
			end
		end
	end
	local backlinks = 0
	for _, n in ipairs(NOTES["tau-agent"]) do
		for _, l in ipairs(n.links) do
			if l.to == "n-0417" then
				backlinks += 1
			end
		end
	end
	t.equal(backlinks >= 2, true, "n-0417's backlinks")
end)

-- The demo run's marks say what it recalled and saved.
t.case("the demo run's view notes the recall, then the two notes saved", function()
	local state = { notes = 8, log = { RECALLED, SAVED } }
	local view = t.run({ repo = "tau-agent", state = state }):view()
	t.equal(view.status.text, "8 notes")
	t.equal(view.notes.m0 ~= nil, true, "the recall")
	t.equal(view.notes.m1 ~= nil, true, "the save")
	t.equal(view.notes.m2, nil)
end)
