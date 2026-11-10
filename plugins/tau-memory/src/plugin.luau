-- tau-memory: a zettelkasten (`docs/reference/plugins.md`, `tau-memory`).
-- Four tools; the index note and a few hits at the start of a run; a
-- memory-only request when compaction drops the conversation; stale
-- marks when the agent edits a file, or a turn's commit changes one,
-- that notes are about; and a pass at the end of a run that keeps what
-- it taught, on by default.
--
-- A run works with its repository's scope and the user's: ids from the
-- user's scope read `user:<id>`. Notes are files, in the plugin's own
-- folders: `tau-memory/` in tau's folder for the repository, and in
-- tau's data folder for the user's.
--
-- What interfaces draw is in `ctx.state`: `recalled`, the notes the
-- run's task found as it started; `log`, each `{ kind = "recalled" |
-- "saved" | "marked" | "error", ... }` in order; `given`, what the
-- conversation has of memory.
local tau = require("tau")
local import = require("import")
local memory = require("memory")
local note = require("note")
local render = require("render")
local ui_import = require("ui_import")
local ui_notes = require("ui_notes")
local ui_settings = require("ui_settings")
local ui = tau.ui

-- Search hits put into a run's context at its start.
local START_HITS = 3

-- Rounds the model gets to search and then write.
local MAX_ROUNDS = 4

-- Tools whose `path` argument names a file they change.
local EDITS = { "edit", "write" }

local USER = memory.USER

local TOOLS = { "memory_write", "memory_search", "memory_read", "memory_link" }

-- The families the settings offer, as the plan names them.
local FAMILIES = { "sol", "luna", "astra", "terra" }

-- The efforts the settings offer; `auto` leaves it to the model.
local EFFORTS = { "auto", "low", "medium", "high" }

local function now_ms(ctx)
	return ctx.now.ms or ctx.now.unix * 1000
end

local function log(ctx, entry)
	ctx.state.log = ctx.state.log or {}
	table.insert(ctx.state.log, entry)
end

-- `id` without any `user:` before it.
local function bare(id)
	while string.sub(id, 1, #USER) == USER do
		id = string.sub(id, #USER + 1)
	end
	return id
end

-- The scope an id names, and the id within it.
local function resolve(scopes, id)
	if string.sub(id, 1, #USER) == USER then
		if scopes.user == nil then
			error("there is no user scope; drop the " .. USER .. " prefix", 0)
		end
		return scopes.user, string.sub(id, #USER + 1)
	end
	if scopes.repository == nil then
		error("there is no repository scope: this run is in no repository", 0)
	end
	return scopes.repository, id
end

local function need_string(args, key)
	if type(args[key]) ~= "string" then
		error("missing field `" .. key .. "`", 0)
	end
	return args[key]
end

local function parse_links(list)
	local links = {}
	for _, link in ipairs(list or {}) do
		if not note.is_link(link.type) then
			error(string.format("%q is not a link type", tostring(link.type)), 0)
		end
		table.insert(links, { to = need_string(link, "to"), type = link.type, why = link.why })
	end
	return links
end

-- `value` with JSON's nulls gone, in it and in the tables it holds: an
-- argument given as null is one not given.
local function without_nulls(value)
	if type(value) ~= "table" then
		return value
	end
	local out = {}
	for key, item in pairs(value) do
		if item ~= json.null then
			out[key] = without_nulls(item)
		end
	end
	return out
end

-- A tool's call, the saves made in a memory request kept as `Saved` was:
-- `{ tool, details, error }` each.
local function saving(name, call)
	return function(args, ctx)
		local ok, answer, details = pcall(call, without_nulls(args or {}), ctx)
		if ctx._memory_saved and name ~= "memory_search" then
			table.insert(ctx._memory_saved, { tool = name, details = ok and details or nil, error = not ok and tostring(answer) or nil })
		end
		if not ok then
			error(answer, 0)
		end
		return answer
	end
end

local function write_call(args, ctx)
	local kind = need_string(args, "type")
	if not note.is_type(kind) then
		error(string.format("%q is not a note type", kind), 0)
	end
	local links = parse_links(args.links)
	-- Notes written in a memory request say who they come from.
	local by = ctx._memory_by or "agent"
	local source = note.source(by)
	source.run = ctx.run.id
	for _, link in ipairs(links) do
		if link.type == "about" then
			table.insert(source.files, link.to)
		end
	end
	local scope_name = args.scope
	if scope_name == nil or scope_name == "repository" or scope_name == "repo" then
		scope_name = "repository"
	elseif scope_name ~= "user" then
		error(string.format("%q is not a scope: repository or user", tostring(scope_name)), 0)
	end
	local scopes = memory.scopes(ctx)
	if scopes[scope_name] == nil then
		error("there is no " .. scope_name .. " scope", 0)
	end
	local draft = {
		type = kind,
		title = need_string(args, "title"),
		description = need_string(args, "description"),
		body = need_string(args, "body"),
		tags = args.tags or {},
		links = links,
		id = args.id and bare(args.id) or nil,
		supersedes = args.supersedes and bare(args.supersedes) or nil,
		source = source,
	}
	local written, why = memory.write(scopes, scope_name, draft, now_ms(ctx))
	if written == nil then
		error(why, 0)
	end
	local prefix = scope_name == "user" and USER or ""
	return render.render_written(written, prefix), { id = prefix .. written.id, action = written.action, redacted = written.redacted }
end

local function search_call(args, ctx)
	local query = need_string(args, "query")
	local limit = math.clamp(math.floor(tonumber(args.limit) or 8), 1, 20)
	local hits = memory.search(memory.scopes(ctx), query, limit)
	local ids = {}
	for _, hit in ipairs(hits) do
		table.insert(ids, hit.id)
	end
	if #hits == 0 then
		return "No notes match.", { ids = ids }
	end
	return render.render_hits(hits), { ids = ids }
end

local function read_call(args, ctx)
	local id = need_string(args, "id")
	local scope, inner = resolve(memory.scopes(ctx), id)
	local reading = scope:read(inner)
	if reading == nil then
		error("no note is called " .. require("store").quoted(id), 0)
	end
	local prefix = string.sub(id, 1, #USER) == USER and USER or ""
	return render.render_reading(reading, prefix)
end

local function link_call(args, ctx)
	local kind = need_string(args, "type")
	if not note.is_link(kind) then
		error(string.format("%q is not a link type", kind), 0)
	end
	local scope, from = resolve(memory.scopes(ctx), need_string(args, "from"))
	local to = bare(need_string(args, "to"))
	local ok, why = scope:link(from, to, kind, args.why, now_ms(ctx))
	if not ok then
		error(why, 0)
	end
	return "Linked `" .. from .. "` " .. kind .. " `" .. to .. "`.", { from = from, to = to, type = kind }
end

local LINK_SCHEMA = {
	type = "object",
	properties = {
		to = { type = "string", description = "A note id, or for `about`, a repository path." },
		type = { type = "string", description = "relates, refines, supersedes, contradicts, derived_from or about." },
		why = { type = "string" },
	},
	required = { "to", "type" },
}

-- What a note is written with: the model the settings name and its
-- effort, or the run's own.
local function writer(ctx)
	local settings, state = ctx.settings, ctx.state
	local own = { model = nil, effort = state.effort }
	if settings.follow_chat then
		return own
	end
	for _, model in ipairs(ctx.models or {}) do
		if model.family == settings.family then
			local effort = settings.reasoning
			if effort == "auto" or not table.find(model.efforts or {}, effort) then
				effort = nil
			end
			return { model = model.id, effort = effort }
		end
	end
	-- A family the plan lacks falls back on the run's model.
	return own
end

-- Asks the model, with only the memory tools and the conversation as
-- text, then carries out the writes it asks for, and keeps what it
-- saved.
local function distill(ctx, messages, prompt, by)
	local with = writer(ctx)
	ctx._memory_saved = {}
	ctx._memory_by = by
	local answer = ctx.tools.infer({
		task = "<conversation>\n" .. render.serialize(messages) .. "\n</conversation>",
		context = json.null,
		instructions = prompt,
		tools = TOOLS,
		wait_for = { "memory_search" },
		rounds = MAX_ROUNDS,
		model = with.model,
		effort = with.effort,
	})
	local saved = ctx._memory_saved
	ctx._memory_saved, ctx._memory_by = nil, nil
	if #saved > 0 then
		log(ctx, { kind = "saved", calls = saved })
	end
	if not answer.ok then
		log(ctx, { kind = "error", message = tostring(answer.error) })
	end
end

-- Marks the notes about `paths`, last written by `written_by`, in both
-- scopes.
local function mark_stale(ctx, paths, why, written_by)
	local scopes = memory.scopes(ctx)
	local marked = {}
	local now = now_ms(ctx)
	for _, name in ipairs({ "repository", "user" }) do
		local scope = scopes[name]
		if scope then
			local ids, err = scope:mark_stale(paths, why, written_by, now)
			if ids == nil then
				log(ctx, { kind = "error", message = err })
			else
				for _, id in ipairs(ids) do
					table.insert(marked, name == "user" and USER .. id or id)
				end
			end
		end
	end
	if #marked > 0 then
		log(ctx, { kind = "marked", notes = marked, why = why })
	end
end

-- What memory reads outside repositories: what Claude Code and Codex
-- kept, when the person imports it (ADR 0044).
local CLAUDE, CODEX = "~/.claude", "~/.codex"

-- What an import reaches, outside any run: the tools' folders, the
-- import's own in memory's, the repositories tau lists, the scopes'
-- notes, and the person's model (`import`'s `env`).
local function env_of(ctx)
	local files = ctx.files
	local roots = files.roots()
	local home = nil
	if roots[CLAUDE] then
		home = string.sub(roots[CLAUDE], 1, #roots[CLAUDE] - #"/.claude")
	elseif roots[CODEX] then
		home = string.sub(roots[CODEX], 1, #roots[CODEX] - #"/.codex")
	end
	local repos = files.repositories and files.repositories() or {}
	local function opened(dir)
		local notes = import.notes(memory.open(files, dir))
		-- Notes keep their times in milliseconds; the import counts
		-- seconds.
		local write = notes.write
		notes.write = function(draft, _now)
			return write(draft, now_ms(ctx))
		end
		return notes
	end
	return {
		files = files,
		home = home,
		dir = roots[memory.DATA] .. "/import",
		now = ctx.now.unix,
		repos = repos,
		scope = function(scope)
			if scope == "you" then
				return opened(roots[memory.DATA])
			end
			local listed = {}
			for _, repo in ipairs(repos) do
				table.insert(listed, {
					name = repo.name,
					folder = repo.folder ~= json.null and repo.folder or nil,
					remote = repo.remote ~= json.null and repo.remote or nil,
				})
			end
			local by = import.claim(scope.place, listed)
			local dir = by and files.repository(by.name)
			if dir == nil then
				return nil
			end
			return opened(dir .. "/" .. memory.FOLDER)
		end,
		infer = ctx.tools and ctx.tools.infer,
		writer = ui_settings.model(ctx.settings, ctx.models),
		signed_in = #(ctx.models or {}) > 0,
	}
end

-- What the setup step chose so far, kept in the import's folder.
local function setup_choice(ctx)
	local path = ctx.files.roots()[memory.DATA] .. "/setup.json"
	local ok, text = pcall(ctx.files.read, path)
	return ui_import.choice(ok and json.decode(text) or nil)
end

-- What keeping proposals last did, for the review page to say.
local function last_kept(ctx)
	local ok, text = pcall(ctx.files.read, ctx.files.roots()[memory.DATA] .. "/import/kept.json")
	return ok and json.decode(text) or nil
end

-- A scope's notebook, as its page lists it.
local function notebook(scope, ctx)
	return scope and ui_notes.notebook(scope.notes, now_ms(ctx)) or {}
end

local plugin = tau.plugin({
	uses = {
		files = { { memory.REPOSITORY, write = true }, { memory.DATA, write = true }, CLAUDE, CODEX },
		infer = true,
		transcript = true,
	},
	settings = {
		schema = {
			type = "object",
			properties = {
				family = { type = "string", enum = FAMILIES },
				reasoning = { type = "string", enum = EFFORTS },
				follow_chat = { type = "boolean" },
				after_each_run = { type = "boolean" },
			},
		},
		-- The newest luna at low effort, and a pass after each run: without
		-- it, a run that never compacts keeps only what the agent wrote.
		default = { family = "luna", reasoning = "low", follow_chat = false, after_each_run = true },
		view = function(settings, ctx)
			return ui_settings.pane(settings, ctx.models)
		end,
	},
	tools = {
		memory_write = {
			description = "Save one idea to long-term memory, for later runs on this repository: a decision and why, a convention, a command, a gotcha (symptom, cause, fix), a case (task, approach, outcome), or a preference the user stated. One idea per note, in full prose. Answers with the nearest existing notes: link to them, or update (id) or supersede (supersedes) one instead of repeating it. Not for task progress or TODOs.",
			parameters = {
				type = "object",
				properties = {
					type = {
						type = "string",
						description = "fact, convention, decision, gotcha, procedure, case, preference, or index (the one map note, loaded into every run)",
					},
					title = { type = "string", description = "The claim, on one line" },
					description = { type = "string", description = "One line for the index and search results" },
					body = { type = "string", description = "The note in full prose: exact versions, flags, paths, error strings" },
					tags = { type = "array", items = { type = "string" } },
					links = { type = "array", items = LINK_SCHEMA },
					id = { type = "string", description = "Update this note (its id) instead of writing a new one" },
					supersedes = { type = "string", description = "Write a new note that replaces this one (its id); the old stays" },
					scope = { type = "string", description = "repository (default) or user, for preferences across projects" },
				},
				required = { "type", "title", "description", "body" },
			},
			call = saving("memory_write", write_call),
		},
		memory_search = {
			description = "Search long-term memory: notes kept from earlier runs on this repository and the user's preferences. Returns ids, titles, descriptions and the matching line, plus notes linked with the best matches. Superseded notes are listed as history; a note marked may be stale was written before a file it is about changed, so check it. Notes are also plain files you can grep.",
			parameters = {
				type = "object",
				properties = {
					query = { type = "string", description = "Words, identifiers, paths or error text to look for" },
					limit = { type = "integer", minimum = 0, description = "At most this many notes (default 8, at most 20)" },
				},
				required = { "query" },
			},
			call = saving("memory_search", search_call),
		},
		memory_read = {
			description = "Read a memory note in full, with its links and the notes that link to it.",
			parameters = {
				type = "object",
				properties = { id = { type = "string", description = "The note's id, as search listed it" } },
				required = { "id" },
			},
			call = saving("memory_read", read_call),
		},
		memory_link = {
			description = "Link two memory notes, typed: relates, refines, supersedes, contradicts or derived_from; or tie a note to a file, crate or symbol with about (the note is then flagged when that file changes). Links are how later searches reach a note that shares none of the query's words.",
			parameters = {
				type = "object",
				properties = {
					from = { type = "string", description = "The note the link starts from" },
					to = { type = "string", description = "A note id, or for `about`, a repository path" },
					type = { type = "string", description = "relates, refines, supersedes, contradicts, derived_from or about" },
					why = { type = "string" },
				},
				required = { "from", "to", "type" },
			},
			call = saving("memory_link", link_call),
		},
	},

	-- The index note and the notes the task finds, unless the conversation
	-- has them already: each message of a chat is a run.
	start = function(plan, ctx)
		local state = ctx.state
		state.effort = plan.effort
		state.model = plan.model
		-- Set by the run's first request.
		state.own_from = nil
		state.turn_since = now_ms(ctx)
		state.recalled = nil
		local scopes = memory.scopes(ctx)
		-- How many notes the repository has, for its line in the run's
		-- plugin list.
		state.notes = #notebook(scopes.repository, ctx)
		local function index_of(scope)
			local n = scope and scope:index_note()
			return n and n.body or nil
		end
		local given = state.given or { notes = {} }
		local hits = {}
		-- A failed search still starts the run, with the index alone.
		local ok, found = pcall(memory.search, scopes, plan.input or "", START_HITS)
		for _, hit in ipairs(ok and found or {}) do
			-- An index note goes in as the index, not as a hit too.
			if hit.type ~= "index" and not table.find(given.notes, hit.id) then
				table.insert(hits, hit)
			end
		end
		local index = render.index_context(index_of(scopes.repository), index_of(scopes.user))
		if given.index == index then
			index = nil
		end
		if index == nil and #hits == 0 then
			return nil
		end
		for _, hit in ipairs(hits) do
			table.insert(given.notes, hit.id)
		end
		if index then
			given.index = index
		end
		state.given = given
		if #hits > 0 then
			local recalled = {}
			for _, hit in ipairs(hits) do
				table.insert(recalled, { id = hit.id, title = hit.title })
			end
			state.recalled = recalled
			log(ctx, { kind = "recalled", notes = recalled })
		end
		return tau.start({ context = { render.start_context(index, hits) } })
	end,

	-- Where the run's own messages start: its input, after the history it
	-- inherited. The pass at its end reads only what the run added.
	before_request = function(request, ctx)
		if ctx.state.own_from == nil then
			ctx.state.own_from = request.messages
		end
		return nil
	end,

	-- An edit to a file notes are about marks them.
	after_tool_result = function(result, ctx)
		if not table.find(EDITS, result.name) then
			return nil
		end
		local path = result.args and result.args.path
		if type(path) ~= "string" then
			return nil
		end
		mark_stale(ctx, { path }, path .. " was edited after this note was written", now_ms(ctx))
		return nil
	end,

	-- A commit the turn left marks the notes about what it changed, those
	-- last written before the turn began.
	turn_end = function(turn, ctx)
		local state = ctx.state
		local written_by = state.turn_since or now_ms(ctx)
		state.turn_since = now_ms(ctx)
		local commit = turn.commit
		if commit == nil or #(commit.paths or {}) == 0 then
			return
		end
		mark_stale(ctx, commit.paths, "commit " .. string.sub(commit.change, 1, 12) .. " changed it", written_by)
	end,

	-- Compaction drops details: a memory request over what it replaced
	-- keeps what is worth keeping first. Pruning, which keeps the
	-- conversation, asks for nothing.
	rewritten = function(info, ctx)
		-- The transcript is new: the pass at the end reads all of it.
		ctx.state.own_from = 1
		if not info.drops_conversation then
			return
		end
		-- The notes given so far go with it: the next run gives them again.
		ctx.state.given = nil
		distill(ctx, ctx.transcript(), render.FLUSH_PROMPT, "agent")
	end,

	-- What a run recalled and saved, in its transcript, and its line in
	-- the run's plugin list.
	view = function(state, ctx)
		local status = ui_notes.status(state.notes)
		return {
			status = status and ui.status(status) or nil,
			notes = ui_notes.marks(state.log, ctx.run.repo),
		}
	end,

	-- The person's notes, across repositories, in the sidebar; each
	-- repository's under it, in its menu, and on ctrl-2 there.
	nav = { label = "Your notes", icon = "memory", page = "notes" },
	repo_nav = { label = "Memory", page = "notes", icon = "memory" },
	repo_menu = { label = "Memory", page = "notes", icon = "memory" },
	keys = { ["ctrl-2"] = "notes" },
	drafts = { ui_notes.SEARCH, ui_import.SEARCH },

	-- What it keeps of the app: the person's notes and what imports found
	-- and propose (ADR 0044).
	app = function(ctx)
		local scopes = memory.scopes(ctx)
		local book = notebook(scopes.user, ctx)
		local imports = import.imports(env_of(ctx))
		return {
			nav = { detail = #book .. " notes" },
			you = { notes = #book, edited = book[1] and book[1].edited or nil },
			imports = imports,
			menu = ui_import.menu(imports),
			choice = setup_choice(ctx),
			kept = last_kept(ctx),
		}
	end,

	-- The Memories step, only when Claude Code or Codex is on this
	-- computer, of the repositories picked so far.
	setup = function(app, ctx)
		if app == nil or app == json.null or app.imports == nil then
			return nil
		end
		local picked = {}
		for _, repo in ipairs(ctx.run.picked or {}) do
			table.insert(picked, {
				name = repo.name,
				folder = repo.folder ~= json.null and repo.folder or nil,
				remote = repo.remote ~= json.null and repo.remote or nil,
			})
		end
		return ui_import.setup(app.imports, picked, ui_import.choice(app.choice))
	end,

	-- What the sidebar's entry and the page of a repository say.
	repo_data = function(ctx)
		local book = notebook(memory.scopes(ctx).repository, ctx)
		local imports = import.imports(env_of(ctx))
		return {
			nav = { detail = #book .. " notes" },
			notes = #book,
			edited = book[1] and book[1].edited or nil,
			banner = (imports.banners or {})[ctx.run.repo],
		}
	end,

	pages = {
		-- A repository's notes, or with no repository, the person's.
		notes = function(params, ctx)
			local scopes = memory.scopes(ctx)
			local imports = import.imports(env_of(ctx))
			if params.repo == nil then
				return ui_notes.page(notebook(scopes.user, ctx), params, { ui_notes.yours() }, ui_import.import_button(imports))
			end
			local top = {}
			local banner = ui_import.banner(params.repo, imports)
			if banner then
				table.insert(top, banner)
			end
			return ui_notes.page(notebook(scopes.repository, ctx), params, top, ui_import.import_button(imports))
		end,
		-- What imports propose, to keep or drop.
		import = function(params, ctx)
			local imports = import.imports(env_of(ctx))
			local query = (params.drafts or {})[ui_import.SEARCH] or ""
			return ui_import.review(imports, ui_import.tab_of(params), query, last_kept(ctx))
		end,
	},

	jobs = {
		[import.JOB] = {
			every = 1,
			label = import.LABEL,
			run = function(ctx)
				import.tick(env_of(ctx))
			end,
		},
	},

	actions = {
		-- What the person asks of an import (`import.act`): what keeping
		-- did is kept for the review page.
		act = function(args, ctx)
			if type(args.act) ~= "table" and type(args.act) ~= "string" then
				return nil
			end
			local answer = import.act(env_of(ctx), args.act)
			if type(args.act) == "table" and args.act.act == "keep" and answer then
				ctx.files.write(ctx.files.roots()[memory.DATA] .. "/import/kept.json", json.encode(answer))
			end
			return nil
		end,
		-- Starts an import, and opens the review page to watch it.
		import = function(args, ctx)
			local request = table.clone(args.request or {})
			request.act = "import"
			import.act(env_of(ctx), request)
			return tau.interface("open", { page = "import" })
		end,
		-- What the setup step chose so far.
		setup_choose = function(args, ctx)
			ctx.files.write(ctx.files.roots()[memory.DATA] .. "/setup.json", json.encode(args.choice or {}))
			return nil
		end,
		-- Starts what the setup step chose, when anything, and ends it.
		setup_start = function(args, ctx)
			if args.request ~= nil and args.request ~= json.null then
				local request = table.clone(args.request)
				request.act = "import"
				import.act(env_of(ctx), request)
			end
			return tau.interface("setup_done")
		end,
	},

	run_end = function(_run, ctx)
		if not ctx.settings.after_each_run then
			return
		end
		local messages = ctx.transcript()
		local own = {}
		for i = ctx.state.own_from or 1, #messages do
			table.insert(own, messages[i])
		end
		distill(ctx, own, render.CONSOLIDATE_PROMPT, "inferred")
	end,
})

return plugin
