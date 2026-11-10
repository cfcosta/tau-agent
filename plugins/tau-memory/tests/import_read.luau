-- Reading what Claude Code and Codex kept, from copies of their files in
-- the test's own: notes, memories, task summaries and chats; the folder
-- each was about, and the remote of the repository there; what is
-- skipped, and what does not read. Reading a remote from a real Git
-- config is the host's, tested in tau-built-in-tests' memory_import.
local tau = require("tau")
local t = tau.test
local g = t.gen
local import = require("import")
local read = require("import_read")

local HOME = "/home/p"

local function lines(events)
	local out = {}
	for _, event in ipairs(events) do
		table.insert(out, json.encode(event) .. "\n")
	end
	return table.concat(out)
end

-- `f(ctx)` in a run of the plugin over `files`.
local function with(files, f, remotes)
	return t.run({ files = files, remotes = remotes or {} }):call(f)
end

local function of(items, tool, material)
	local out = {}
	for _, item in ipairs(items) do
		if item.tool == tool and item.material == material then
			table.insert(out, item)
		end
	end
	return out
end

local function name_of(path)
	return string.match(path, "([^/]+)$")
end

-- A note as Claude Code writes it, with its type under `metadata` or at
-- the top.
local function note_file(name, description, kind, body, nested)
	local typed = nested and ("metadata:\n  node_type: memory\n  type: " .. kind .. "\n  originSessionId: s-1\n")
		or ("type: " .. kind .. "\n")
	return "---\nname: " .. name .. "\ndescription: " .. description .. "\n" .. typed .. "---\n\n" .. body .. "\n"
end

t.property("every folder Claude Code names reads back as that folder, while it is there", function(draw)
	local parts = draw(g.lists(g.from_regex("[a-zA-Z0-9][a-zA-Z0-9._-]{0,8}"), { min = 1, max = 4 }))
	local path = "/base/" .. table.concat(parts, "/")
	local found = with({ [path .. "/.keep"] = "" }, function(ctx)
		return read.decode(ctx.files, read.encode(path))
	end)
	t.equal(found, path)
end)

t.case("a folder that is gone decodes to nothing", function()
	local found = with({ ["/base/other/.keep"] = "" }, function(ctx)
		return read.decode(ctx.files, read.encode("/base/was-here/once"))
	end)
	t.equal(found, nil)
end)

t.property("any well-formed note reads as itself", function(draw)
	local word = g.from_regex("[A-Za-z0-9][A-Za-z0-9 ,.()/`-]{0,30}[A-Za-z0-9]")
	local name = draw(word)
	local description = draw(word)
	local body = draw(g.from_regex("[A-Za-z0-9 .,\n-]{1,120}"))
	local kind = draw(g.sampled_from({ "user", "feedback", "project", "reference" }))
	local nested = draw(g.booleans())
	local note = read.parse_note(note_file(name, description, kind, body, nested), "memory/x.md")
	t.equal(note.title, read.one_line(name))
	t.equal(note.description, read.one_line(description))
	t.equal(note.body, read.trim(body))
	local about_you = kind == "user" or kind == "feedback"
	t.equal(note.about_you, about_you)
	t.equal(note.kind, about_you and "preference" or "fact")
end)

t.case("a note reads what it can", function()
	t.equal(read.parse_note("just text\n", "a.md"), nil)
	t.equal(read.parse_note("---\nname: x\n", "a.md"), nil)
	local note = read.parse_note(
		'---\ndescription: "Quoted: with a colon"\nmetadata:\n  type: project\n---\nBody.\n',
		"memory/keep-target-dirs_apart.md"
	)
	t.equal(note.title, "keep target dirs apart")
	t.equal(note.description, "Quoted: with a colon")
	local folded = read.parse_note(
		"---\nname: Folded\ndescription: >-\n  one line\n  and another\ntype: reference\n---\nB\n",
		"a.md"
	)
	t.equal(folded.description, "one line and another")
	t.equal(folded.kind, "fact")
end)

-- A home with Claude Code's files: two projects, one whose chats say
-- where it ran and one only its name says.
local REPO = HOME .. "/work/shop-api"
local OTHER = HOME .. "/work/notes.site"
local function claude_home()
	local first = HOME .. "/.claude/projects/" .. read.encode(REPO)
	return {
		[REPO .. "/src/main.rs"] = "",
		[OTHER .. "/index.md"] = "",
		[first .. "/memory/run-tests-with-make.md"] = note_file(
			"Tests run with make check",
			"Not cargo test",
			"project",
			"Run `make check`.",
			true
		),
		[first .. "/memory/terse-answers.md"] = note_file(
			"Answers are short",
			"The person wants terse replies",
			"feedback",
			"Keep it short.",
			false
		),
		[first .. "/memory/MEMORY.md"] = "- [Tests](run-tests-with-make.md)\n",
		[first .. "/memory/broken.md"] = "no front matter here\n",
		[first .. "/memory/data/table.tsv"] = "a\tb\n",
		[first .. "/0001.jsonl"] = lines({
			{ type = "permission-mode", sessionId = "0001" },
			{
				type = "user",
				cwd = REPO .. "/src",
				timestamp = "2026-10-04T10:00:00Z",
				message = { role = "user", content = "Why do the tests hang?" },
			},
		}),
		-- A sub-agent's chat sits deeper, and is not one of the folder's.
		[first .. "/0001/subagents/a.jsonl"] = "{}\n",
		-- The other folder has notes and no chat: its name says where.
		[HOME .. "/.claude/projects/" .. read.encode(OTHER) .. "/memory/site-builds-with-zola.md"] = note_file(
			"The site builds with zola",
			"zola build",
			"reference",
			"zola build",
			true
		),
	}
end

t.case("Claude Code's scan finds its notes and chats and their folders", function()
	local scan = with(claude_home(), function(ctx)
		return read.scan(ctx.files, HOME)
	end, { [REPO .. "/src"] = "git@github.com:acme/shop-api.git" })
	local saved = of(scan.items, read.CLAUDE, read.SAVED)
	local names = {}
	for _, item in ipairs(saved) do
		table.insert(names, name_of(item.path))
	end
	-- By folder: `notes.site` before `shop-api`.
	t.equal(names, { "site-builds-with-zola.md", "run-tests-with-make.md", "terse-answers.md" })
	saved = { saved[2], saved[3], saved[1] }
	-- The chat ran in a folder inside the repository; the project is named
	-- after where it started.
	local place = saved[1].place
	t.equal(place.remote, "git@github.com:acme/shop-api.git")
	t.equal(import.matches(place, REPO, nil), true)
	t.equal(saved[2].about_you, true)
	t.equal(saved[1].about_you, false)
	t.equal(saved[3].place, { folder = OTHER })
	t.equal(#scan.failed, 1)
	t.equal(string.sub(scan.failed[1], -#"memory/broken.md"), "memory/broken.md")
	local chats = of(scan.items, read.CLAUDE, read.CHAT)
	t.equal(#chats, 1)
	t.equal(chats[1].date, "2026-10-04")
	t.equal(chats[1].session, "0001")
	t.equal(read.show(HOME, chats[1].path), "~/.claude/projects/" .. read.encode(REPO) .. "/0001.jsonl")
end)

t.case("a Claude Code chat reads as text, and again from where it stopped", function()
	local first = lines({
		{
			type = "user",
			message = { role = "user", content = "Pin the toolchain.<system-reminder>internal</system-reminder>" },
		},
		{
			type = "assistant",
			message = {
				content = {
					{ type = "thinking", thinking = "hidden" },
					{ type = "text", text = "Pinning it to 1.92." },
					{ type = "tool_use", name = "edit", input = { path = "rust-toolchain.toml" } },
				},
			},
		},
		{ type = "user", message = { content = { { type = "tool_result", content = { { type = "text", text = "ok" } } } } } },
		{ type = "user", isMeta = true, message = { content = "meta" } },
		{ type = "assistant", isSidechain = true, message = { content = { { type = "text", text = "side" } } } },
	})
	local more = lines({ { type = "user", message = { content = "And the lints." } } })
	local path = "/c/chat.jsonl"
	local function chat(text)
		return { tool = read.CLAUDE, material = read.CHAT, path = path, size = #text }
	end
	-- A line still being written is not read.
	local partial = first .. '{"type": "us'
	local got = with({ [path] = partial }, function(ctx)
		return read.item(ctx.files, chat(partial), 0)
	end)
	t.equal(
		got.text,
		"[user]\nPin the toolchain.\n\n[assistant]\nPinning it to 1.92.\n\n"
			.. '[call edit] {"path":"rust-toolchain.toml"}\n\n[result]\nok\n\n'
	)
	t.equal(got.offset, #first)
	local again = with({ [path] = first .. more }, function(ctx)
		return read.item(ctx.files, chat(first .. more), got.offset)
	end)
	t.equal(again.text, "[user]\nAnd the lints.\n\n")
	t.equal(again.offset, #first + #more)
	local bad, why = with({ [path] = "not json\n" }, function(ctx)
		return read.item(ctx.files, chat("not json\n"), 0)
	end)
	t.equal(bad, nil)
end)

-- A home with Codex's files.
local BILLING = HOME .. "/work/billing"
local function codex_home()
	local memories = HOME .. "/.codex/memories"
	local function meta(id, extra)
		local payload = { id = id, cwd = BILLING, timestamp = "2026-10-05T09:00:00Z", thread_source = "user" }
		for key, value in pairs(extra) do
			payload[key] = value
		end
		return { timestamp = "2026-10-05T09:00:00Z", type = "session_meta", payload = payload }
	end
	local sessions = HOME .. "/.codex/sessions/2026/10/05"
	return {
		[BILLING .. "/.keep"] = "",
		[memories .. "/MEMORY.md"] = "# Task Group: "
			.. BILLING
			.. " — invoices and their rounding\n\nscope: billing\n\n## Task 1: Round once\n\n### rollout_summary_files\n\n- rollout_summaries/2026-10-01-a.md (cwd="
			.. BILLING
			.. ")\n\n## User preferences\n\n- Keep money in cents.\n\n# Task Group: /nowhere/else — another folder\n\n## Reusable knowledge\n\n- Something else.\n",
		[memories .. "/memory_summary.md"] = "## User Profile\n\nPrefers small commits.\n",
		[memories .. "/rollout_summaries/2026-10-01-a.md"] = "thread_id: t-a\nupdated_at: 2026-10-01T10:00:00Z\ncwd: "
			.. BILLING
			.. "\n\n# Round once\n",
		[memories .. "/rollout_summaries/2026-10-02-b.md"] = "thread_id: t-b\nupdated_at: 2026-10-02T10:00:00Z\ncwd: "
			.. BILLING
			.. "\n\n# Taxes\n",
		[memories .. "/raw_memories.md"] = "# Raw Memories\n\n## Thread `t-a`\nupdated_at: 2026-10-01T10:00:00Z\ncwd: "
			.. BILLING
			.. "\n\nsame as its summary\n\n## Thread `t-c`\nupdated_at: 2026-10-03T10:00:00Z\ncwd: "
			.. BILLING
			.. "\n\nonly here\n",
		[sessions .. "/rollout-own.jsonl"] = lines({ meta("s-own", {}) }),
		[sessions .. "/rollout-copy.jsonl"] = lines({ meta("s-copy", {}) }),
		[sessions .. "/rollout-sub.jsonl"] = lines({ meta("s-sub", { thread_source = "subagent" }) }),
		[sessions .. "/rollout-bad.jsonl"] = "not json\n",
		[HOME .. "/.codex/" .. read.IMPORTS] = json.encode({
			records = { { imported_thread_id = "s-copy", source_path = "/x.jsonl" } },
		}),
	}
end

t.case("Codex's scan reads each thing once", function()
	-- The jj repository's Git store says where it pushes.
	local scan = with(codex_home(), function(ctx)
		return read.scan(ctx.files, HOME)
	end, { [BILLING] = "https://github.com/acme/billing" })
	local saved = of(scan.items, read.CODEX, read.SAVED)
	local keys = {}
	for _, item in ipairs(saved) do
		table.insert(keys, read.show(HOME, item.key))
	end
	t.equal(keys, {
		"~/.codex/memories/MEMORY.md#0",
		"~/.codex/memories/MEMORY.md#1",
		"~/.codex/memories/memory_summary.md",
		"~/.codex/memories/rollout_summaries/2026-10-02-b.md",
		"~/.codex/memories/raw_memories.md#t-c",
	})
	t.equal(import.matches(saved[1].place, BILLING, nil), true)
	t.equal(string.find(saved[1].section, "Keep money in cents", 1, true) ~= nil, true)
	t.equal(saved[2].place.folder, "/nowhere/else")
	t.equal(saved[3].place, nil)
	t.equal(saved[3].about_you, true)
	t.equal(saved[4].session, "t-b")
	t.equal(saved[4].date, "2026-10-02")
	t.equal(import.matches(saved[4].place, nil, "git@github.com:acme/billing.git"), true)
	local chats = of(scan.items, read.CODEX, read.CHAT)
	local sessions = {}
	for _, item in ipairs(chats) do
		table.insert(sessions, item.session)
	end
	t.equal(sessions, { "s-own" })
	t.equal(chats[1].date, "2026-10-05")
	t.equal(#scan.failed, 1)
	t.equal(name_of(scan.failed[1]), "rollout-bad.jsonl")
end)

t.case("a Codex session reads as text", function()
	local function message(role, kind, text)
		return {
			type = "response_item",
			payload = { type = "message", role = role, content = { { type = kind, text = text } } },
		}
	end
	local text = lines({
		{ type = "session_meta", payload = { id = "s" } },
		message("developer", "input_text", "rules"),
		message("user", "input_text", "<environment_context>\n<cwd>/x</cwd>\n</environment_context>"),
		message("user", "input_text", "# AGENTS.md instructions for /x"),
		message("user", "input_text", "Make rounding happen once."),
		{ type = "response_item", payload = { type = "function_call", name = "shell", arguments = '{"cmd":"ls"}' } },
		{ type = "response_item", payload = { type = "function_call_output", output = "src\n" } },
		{ type = "response_item", payload = { type = "reasoning", summary = array({}) } },
		message("assistant", "output_text", "Done: rounding is in one place."),
		{ type = "event_msg", payload = { type = "token_count" } },
	})
	local got = with({ ["/s.jsonl"] = text }, function(ctx)
		return read.item(ctx.files, { tool = read.CODEX, material = read.CHAT, path = "/s.jsonl", size = #text }, 0)
	end)
	t.equal(
		got.text,
		'[user]\nMake rounding happen once.\n\n[call shell] {"cmd":"ls"}\n\n'
			.. "[result]\nsrc\n\n[assistant]\nDone: rounding is in one place.\n\n"
	)
end)

t.case("a home with neither tool has nothing", function()
	local scan = with({ [HOME .. "/notes.txt"] = "" }, function(ctx)
		return read.scan(ctx.files, HOME)
	end)
	t.equal(#scan.items, 0)
	t.equal(#scan.failed, 0)
end)

t.property("a remote is one remote however it is written", function(draw)
	local owner = draw(g.from_regex("[a-z0-9][a-z0-9-]{0,8}"))
	local name = draw(g.from_regex("[a-z0-9][a-z0-9._-]{0,8}[a-z0-9]"))
	local host = draw(g.sampled_from({ "github.com", "GitHub.com", "gitlab.example.org" }))
	local suffix = draw(g.sampled_from({ "", ".git", "/", ".git/" }))
	local forms = {
		"git@" .. host .. ":" .. owner .. "/" .. name .. suffix,
		"ssh://git@" .. host .. "/" .. owner .. "/" .. name .. suffix,
		"ssh://git@" .. host .. ":22/" .. owner .. "/" .. name .. suffix,
		"https://" .. host .. "/" .. owner .. "/" .. name .. suffix,
		"https://someone@" .. host .. "/" .. owner .. "/" .. name .. suffix,
	}
	local want = string.lower(host) .. "/" .. owner .. "/" .. name
	for _, form in ipairs(forms) do
		t.equal(import.normalize_remote(form), want, form)
		t.equal(import.matches({ remote = form }, nil, "https://" .. host .. "/" .. owner .. "/" .. name), true)
	end
	local other = "https://" .. host .. "/" .. owner .. "/" .. name .. "-fork"
	t.equal(import.matches({ remote = forms[1] }, nil, other), false)
end)

t.case("a place is the repository's that holds it", function()
	local place = { folder = "/work/shop/api" }
	t.equal(import.matches(place, "/work/shop", nil), true)
	t.equal(import.matches(place, "/work/shop/api", nil), true)
	t.equal(import.matches(place, "/work/sho", nil), false)
	t.equal(import.matches(place, nil, "https://github.com/a/b"), false)
	t.equal(import.normalize_remote(""), nil)
	t.equal(import.normalize_remote("/srv/git/app.git"), "/srv/git/app")
end)

t.property("chunks join into the text, none longer than asked", function(draw)
	local text = draw(g.text({ max = 400 }))
	local max = draw(g.integers({ min = 1, max = 80 }))
	local parts = read.chunks(text, max)
	t.equal(table.concat(parts), text)
	for _, part in ipairs(parts) do
		t.equal(#part <= math.max(max, 4), true, part)
		t.equal(#part > 0, true)
	end
end)
