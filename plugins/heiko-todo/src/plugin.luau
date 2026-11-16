-- heiko-todo (ADR 0048): issues a repository keeps for its runs, like
-- beads, and each run's own steps. Runs have beads' whole tool set; a
-- claim is a run's and ends with it; the person works the issues on a
-- board, a list or a graph of each repository, on the computer or a
-- phone.
local heiko = require("heiko")
local ui = heiko.ui
local steps = require("steps")
local graph = require("graph")
local issues = require("issues")
local beads = require("beads")
local time = require("time")

local TONE = { [0] = "bad", [1] = "warn", [2] = "info", [3] = "neutral", [4] = "neutral" }

-- A priority on a square, outlined in its tone; the last two are quiet.
local function priority(issue)
	local tone = TONE[issue.priority]
	return ui.square("P" .. issue.priority, tone ~= "neutral" and "outlined" or "quiet", tone)
end

-- A done-when criterion, or a step, as a square: ticked once done, else
-- `label`, outlined while it is the one worked on.
local function check(done, label, current)
	if done then
		return ui.square("✓", "filled", "good")
	end
	return ui.square(label or "", current and "outlined" or "quiet", "info")
end

-- What a run is told of the issues, as it starts.
local PROMPT = [[This repository keeps issues for its runs, with the todo_* tools. Before choosing what to work on, ask todo_ready; claim the issue you work on with todo_claim, and check its "done when" criteria as you meet them. When you find work outside the issue, file it with todo_create rather than drift into it. Close what you finish with todo_close and the reason; what you leave open goes back for the next run when yours ends. Keep your own plan with todo_steps. To hand an issue to another chat, ask with todo_dispatch: the person allows it.]]

-- The state that changes with every write: what a page of the
-- repository draws again from.
local function bump(store)
	store.put("rev", (store.get("rev") or 0) + 1)
end

-- The plugin's store, as the issues module reads it: each read awaited.
-- A hook's writes are kept until it ends, then given together, in its
-- last step (`atomic`): the host commits what one step gives whole, and
-- a change that reads between its writes would else be cut in two by a
-- crash, or keep half of itself when it fails. Its own reads see them.
local cmd = heiko.cmd
local DELETED = {}
local pending = setmetatable({}, { __mode = "k" })

local function writes()
	return pending[coroutine.running()]
end

local function give(key, value)
	if value == DELETED then
		heiko.give(cmd.store.delete(key))
	else
		heiko.give(cmd.store.put(key, value))
	end
end

local function write(key, value)
	local w = writes()
	if w == nil then
		give(key, value)
		return
	end
	if w.values[key] == nil then
		table.insert(w.order, key)
	end
	w.values[key] = value
end

local STORE = {
	get = function(key)
		local w = writes()
		local mine = w and w.values[key]
		if mine == DELETED then
			return nil
		elseif mine ~= nil then
			return heiko.values.copy(mine)
		elseif w and w.read[key] ~= nil then
			return heiko.values.copy(w.read[key] or nil)
		end
		return heiko.await(cmd.store.get(key))
	end,
	list = function(prefix)
		local listed = heiko.await(cmd.store.list(prefix)) or {}
		local w = writes()
		if w == nil then
			return listed
		end
		local out, seen = {}, {}
		for _, kept in ipairs(listed) do
			seen[kept.key] = true
			local mine = w.values[kept.key]
			if mine == nil then
				table.insert(out, kept)
			elseif mine ~= DELETED then
				table.insert(out, { key = kept.key, value = heiko.values.copy(mine) })
			end
		end
		for _, key in ipairs(w.order) do
			local mine = w.values[key]
			if not seen[key] and mine ~= DELETED and string.sub(key, 1, #prefix) == prefix then
				table.insert(out, { key = key, value = heiko.values.copy(mine) })
			end
		end
		table.sort(out, function(x, y)
			return x.key < y.key
		end)
		return out
	end,
	put = function(key, value)
		write(key, heiko.values.copy(value))
	end,
	delete = function(key)
		write(key, DELETED)
	end,
}

-- How many times a hook tries again when another run changed the issues
-- while it read and decided.
local TRIES = 8

-- `f` as a hook whose writes go in together as it ends, or none of them
-- when it fails: one commit, while `rev` is what it read first (ADR
-- 0056). Every change bumps `rev`, so a commit that finds it moved finds
-- another run's change since this one read, and writes nothing: the hook
-- runs again, on the model as it was and the store as it is now. One
-- that fails with `rev` moved may have read half before that change and
-- half after, and failed on what no order of the two gives: it runs
-- again too, and fails only on the store as one change left it.
local function atomic(f)
	return function(input, model, env)
		local thread = coroutine.running()
		if pending[thread] ~= nil then
			return f(input, model, env)
		end
		local before = json.encode(model)
		for _ = 1, TRIES do
			local rev = heiko.await(cmd.store.get("rev"))
			pending[thread] = { values = {}, order = {}, read = { rev = if rev == nil then false else rev } }
			local results = table.pack(pcall(f, input, model, env))
			local w = pending[thread]
			pending[thread] = nil
			if not results[1] then
				if heiko.await(cmd.store.get("rev")) == rev then
					error(results[2], 0)
				end
			else
				if #w.order == 0 then
					return table.unpack(results, 2, results.n)
				end
				local put, delete = {}, array({})
				for _, key in ipairs(w.order) do
					local value = w.values[key]
					if value == DELETED then
						table.insert(delete, key)
					else
						put[key] = value
					end
				end
				local expect = { rev = if rev == nil then json.null else rev }
				if heiko.await(cmd.store.commit({ expect = expect, put = put, delete = delete })) then
					return table.unpack(results, 2, results.n)
				end
			end
			-- Another run's change came first: the model as it was, in
			-- place, and again.
			local was = json.decode(before)
			table.clear(model)
			for key, value in pairs(was) do
				model[key] = value
			end
		end
		error("the issues kept changing under this call; try it again", 0)
	end
end

local function store_of()
	return STORE
end

-- One line of an issue, for the model.
local function brief(issue)
	local done, all = graph.checked(issue)
	local out = {
		id = issue.id,
		title = issue.title,
		type = issue.type,
		priority = "P" .. issue.priority,
		status = issue.status,
	}
	if all > 0 then
		out.criteria = done .. "/" .. all
	end
	if issue.claim then
		out.claim = issue.claim
	end
	if #(issue.labels or {}) > 0 then
		out.labels = issue.labels
	end
	return out
end

local function briefs(all, ids)
	local out = array({})
	for _, id in ipairs(ids) do
		table.insert(out, brief(all[id]))
	end
	return out
end

-- An issue whole, with what it links to and what links to it.
local function whole(all, issue, at)
	local out = brief(issue)
	out.description = issue.description
	out.criteria = array({})
	for n, criterion in ipairs(issue.criteria or {}) do
		table.insert(out.criteria, { n = n, text = criterion.text, done = criterion.done })
	end
	out.links = array({})
	for _, link in ipairs(issue.links or {}) do
		table.insert(out.links, { kind = link.kind, id = link.id })
	end
	out.blocked_by = array(graph.blockers(all, issue.id))
	out.blocks = array(graph.blocking(all, issue.id))
	if issue.type == "epic" then
		local done, total = graph.progress(all, issue.id)
		out.children = array(graph.children(all, issue.id))
		out.progress = done .. "/" .. total
	end
	out.comments = array({})
	for _, comment in ipairs(issue.comments or {}) do
		table.insert(out.comments, { at = time.iso(comment.at), by = comment.by, text = comment.text })
	end
	out.ready = graph.is_ready(all, issue.id, at)
	out.blocked = issue.blocked
	out.close_reason = issue.close_reason
	if issue.defer_until then
		out.defer_until = time.iso(issue.defer_until)
	end
	return out
end

local function filter_of(args)
	return {
		label = args.label,
		type = args.type,
		priority = args.priority and issues.priority(args.priority) or nil,
	}
end

-- The issue the run holds, the first of them.
local function held(store, run)
	return issues.held_by(store, run)[1]
end

-- A change's answer: what the model reads, the line its card shows, and
-- what undoes it.
local function changed(store, issue, line, before)
	bump(store)
	return { issue = brief(issue), line = line, undo = before and { restore = before } or nil }
end

local function snapshot(store, id)
	return issues.all(store)[id]
end

-- What a new chat on `issue` starts with.
local function opening(issue)
	local lines = { "Work on " .. issue.id .. ": " .. issue.title .. "." }
	if (issue.description or "") ~= "" then
		table.insert(lines, "")
		table.insert(lines, issue.description)
	end
	if #(issue.criteria or {}) > 0 then
		table.insert(lines, "")
		table.insert(lines, "Done when:")
		for _, criterion in ipairs(issue.criteria) do
			table.insert(lines, (criterion.done and "- [x] " or "- [ ] ") .. criterion.text)
		end
	end
	table.insert(lines, "")
	table.insert(lines, "It is claimed for this chat; close it with todo_close when it is done.")
	return table.concat(lines, "\n")
end

-- Starts a chat on `id` with it claimed: held for the chat until it
-- starts, so no run takes it meanwhile.
local function start_chat(store, at, id)
	local issue = issues.find(issues.all(store), id)
	if issue.claim and issue.claim ~= "starting" then
		return id .. " is held by the run " .. issue.claim
	end
	if issue.claim == "starting" then
		return id .. " is being started in a chat already"
	end
	if issue.status == "closed" then
		issues.reopen(store, at, id)
		issue = issues.find(issues.all(store), id)
	end
	issues.claim(store, at, id, "starting")
	bump(store)
	return heiko.start_chat({ text = opening(issue), ["then"] = "started", args = { issue = id } })
end

-- The step at `at` of the run's, done and naming `id`.
local function promote(state, at, id)
	local list = state.steps or {}
	local step = list[tonumber(at) or 0]
	if step then
		step.status = "done"
		step.issue = id
	end
end

local function string_list(desc)
	return { type = "array", items = { type = "string" }, description = desc }
end

local PRIORITY = { type = "string", enum = { "P0", "P1", "P2", "P3", "P4" }, description = "P0 drop everything, to P4 some day; P2 by default." }
local TYPE = { type = "string", enum = { "bug", "feature", "task", "epic", "chore" } }
local ID = { type = "string", description = "The issue's id, such as td-4e1b." }

local function object(properties, required)
	return { type = "object", properties = properties, required = array(required or {}), additionalProperties = false }
end

-- A change's card: its line, and an undo.
local function change_card(input, model, env)
	local result = input.result
	local line = ui.text(result.line or "")
	if not result.undo then
		return line
	end
	return ui.row({ line, ui.button("undo", "undo", result.undo, "secondary") })
end

-- The run's note of the issue `id`, if it has one. A run's notes
-- (`model.notes`, oldest first) are the issues it filed, each with what
-- the run did to it since and the calls that did it: the transcript
-- draws one note for them all, where the last of the calls is.
local function note_of(model, id)
	for _, note in ipairs(model.notes or {}) do
		if note.id == id then
			return note
		end
	end
	return nil
end

-- Folds the call going on into the note of `issue`, when the run has
-- one: `said` joins what happened to it, and `undo` is what its undo
-- now undoes. Gives the note.
local function fold(model, env, issue, said, undo)
	local note = note_of(model, issue.id)
	if not note then
		return nil
	end
	note.title = issue.title
	if said and note.said[#note.said] ~= said then
		table.insert(note.said, said)
	end
	if undo then
		note.undo = undo
	end
	table.insert(note.calls, env.run.call)
	return note
end

-- An issue's note: under its mark, what the run did to it so far with
-- its id and the undo, and its title on a slip.
local function note_tree(note)
	local head = { ui.toned(table.concat(note.said, " · "), "neutral", true), ui.spacer(), ui.toned(note.id, "neutral", true) }
	if note.undo then
		table.insert(head, ui.button("undo", "undo", note.undo, "secondary"))
	end
	return ui.row({ ui.kanji("件", "good"), ui.stack({ ui.row(head), ui.slip(note.title, "good") }) })
end

-- The line a list of steps leaves in the chat once all are done: how
-- many, a square for each, and the steps behind it.
local function finished_line(call, list)
	local inks, lines = {}, {}
	for _, step in ipairs(list) do
		table.insert(inks, "good")
		local line = step.issue and (step.text .. " → " .. step.issue) or step.text
		table.insert(lines, ui.row({ ui.square("✓", "filled", "good"), ui.toned(line, "neutral") }))
	end
	local count = heiko.count.plural(#list, "step")
	return ui.section("steps-" .. call, "", lines, { small = true, lead = { ui.toned(count, "good", true), ui.pips(inks) } })
end

-- How the calls the run's notes fold draw: the last call about an issue
-- as its note, with no line of its own, and the ones before it as
-- nothing at all. A call that set the run's steps draws nothing either:
-- they are over the composer while any is left, and the call that
-- finished a list is the line it leaves.
local function folded(model)
	local calls = nil
	for _, note in ipairs(model.notes or {}) do
		for at, call in ipairs(note.calls) do
			calls = calls or {}
			calls[call] = { head = ui.stack({}), body = at == #note.calls and note_tree(note) or nil }
		end
	end
	for _, call in ipairs(model.step_calls or {}) do
		calls = calls or {}
		calls[call] = { head = ui.stack({}) }
	end
	for _, done in ipairs(model.finished or {}) do
		calls[done.call].body = finished_line(done.call, done.steps)
	end
	return calls
end

-- A read's card: the issues it found.
local function read_card(input, model, env)
	local result = input.result
	local found = result.issues or result.ready or result.blocked or {}
	if result.issue and not result.line then
		found = { result.issue }
	end
	if #found == 0 then
		return ui.text("No issues.")
	end
	local rows = {}
	for _, issue in ipairs(found) do
		table.insert(rows, { issue.id, issue.priority or "", issue.title or "", issue.status or "" })
	end
	return ui.table({ "Id", "P", "Title", "Status" }, rows)
end

local tools = {
	todo_ready = {
		kind = "look",
		description = "The ready issues: open, not deferred, blocked by nothing open, sorted by priority then age. Ask before choosing work. With blocked, what waits on what instead.",
		parameters = object({
			label = { type = "string" },
			type = TYPE,
			priority = PRIORITY,
			blocked = { type = "boolean", description = "List what is blocked, and on what." },
		}),
		call = function(args, model, env)
			local all = issues.all(store_of())
			local filter = filter_of(args)
			if args.blocked then
				local out = array({})
				for _, id in ipairs(graph.blocked(all, heiko.now(env), filter)) do
					local one = brief(all[id])
					one.waits_on = array(graph.blockers(all, id))
					one.blocked = all[id].blocked
					table.insert(out, one)
				end
				return { blocked = out }
			end
			return { ready = briefs(all, graph.ready(all, heiko.now(env), filter)) }
		end,
		card = read_card,
	},
	todo_list = {
		kind = "look",
		description = "Issues by status, label, type, priority or claim; with tree, each epic's children under it.",
		parameters = object({
			status = { type = "string", enum = { "open", "in_progress", "blocked", "deferred", "closed" } },
			label = { type = "string" },
			type = TYPE,
			priority = PRIORITY,
			claim = { type = "string", description = "The run that holds them." },
			tree = { type = "boolean" },
		}),
		call = function(args, model, env)
			local all = issues.all(store_of())
			local filter = filter_of(args)
			local ids = {}
			for id, issue in pairs(all) do
				if
					graph.matches(issue, filter)
					and (args.status == nil or issue.status == args.status)
					and (args.claim == nil or issue.claim == args.claim)
				then
					table.insert(ids, id)
				end
			end
			graph.by_priority(all, ids)
			if not args.tree then
				return { issues = briefs(all, ids) }
			end
			local out = array({})
			for _, id in ipairs(ids) do
				if #graph.links(all[id], "parent") == 0 then
					local one = brief(all[id])
					if all[id].type == "epic" then
						one.children = briefs(all, graph.children(all, id))
					end
					table.insert(out, one)
				end
			end
			return { issues = out }
		end,
		card = read_card,
	},
	todo_show = {
		kind = "look",
		description = "One issue whole: its description, criteria, links, what blocks it and its comments.",
		parameters = object({ id = ID }, { "id" }),
		call = function(args, model, env)
			local all = issues.all(store_of())
			return { issue = whole(all, issues.find(all, args.id), heiko.now(env)) }
		end,
		card = read_card,
	},
	todo_search = {
		kind = "search",
		description = "Issues whose title, description or comments hold the text, any case.",
		parameters = object({ text = { type = "string" } }, { "text" }),
		call = function(args, model, env)
			local all = issues.all(store_of())
			return { issues = briefs(all, issues.search(all, args.text)) }
		end,
		card = read_card,
	},
	todo_create = {
		kind = "change",
		description = "Files a new issue. What you find outside your issue goes here rather than into your work: it is found while the issue you hold. Give the step it comes from, when it is one of yours, and the step is done, naming it.",
		parameters = object({
			title = { type = "string" },
			description = { type = "string", description = "Markdown." },
			type = TYPE,
			priority = PRIORITY,
			labels = string_list("Labels, such as phone."),
			criteria = string_list("What done means, a line each."),
			parent = { type = "string", description = "The epic it is part of." },
			step = { type = "integer", description = "Your step it comes from, from 1." },
		}, { "title" }),
		call = function(args, model, env)
			local store = store_of()
			local args2 = table.clone(args)
			args2.found_while = held(store, env.run.id)
			local issue = issues.create(store, heiko.now(env), args2)
			if args.step then
				promote(model, args.step, issue.id)
			end
			bump(store)
			local undo = { forget = issue.id }
			model.notes = model.notes or array({})
			table.insert(model.notes, { id = issue.id, title = issue.title, said = array({ "filed" }), calls = array({ env.run.call }), undo = undo })
			return { issue = brief(issue), line = "filed " .. issue.id .. ": " .. issue.title, undo = undo }
		end,
		card = change_card,
	},
	todo_update = {
		kind = "change",
		description = "Changes an issue's title, description, type, priority, labels, criteria (replaced, unchecked) or status. A blocked status says what outside heiko it waits on; to wait on an issue, link it.",
		parameters = object({
			id = ID,
			title = { type = "string" },
			description = { type = "string" },
			type = TYPE,
			priority = PRIORITY,
			labels = string_list("Labels."),
			criteria = string_list("What done means, a line each."),
			status = { type = "string", enum = { "open", "in_progress", "blocked", "deferred" } },
			blocked = { type = "string", description = "With status blocked: what it waits on." },
		}, { "id" }),
		call = function(args, model, env)
			local store = store_of()
			local before = snapshot(store, args.id)
			local issue = issues.update(store, heiko.now(env), args)
			local line = issue.id .. " changed"
			if before and before.priority ~= issue.priority then
				line = issue.id .. ": P" .. before.priority .. " to P" .. issue.priority
			elseif before and before.status ~= issue.status then
				line = issue.id .. ": " .. before.status .. " to " .. issue.status
			end
			local note = note_of(model, issue.id)
			if note then
				note.title = issue.title
			end
			return changed(store, issue, line, before)
		end,
		card = change_card,
	},
	todo_claim = {
		kind = "change",
		description = "Claims an issue for this run: it goes in progress, yours until it is closed or your run ends. Refused when another run holds it.",
		parameters = object({ id = ID }, { "id" }),
		call = function(args, model, env)
			local store = store_of()
			local issue = issues.claim(store, heiko.now(env), args.id, env.run.id)
			model.issue = issue.id
			local note = fold(model, env, issue, "claimed by this run")
			if note then
				note.claimed = true
			end
			bump(store)
			return { issue = brief(issue), line = "claimed " .. issue.id }
		end,
		card = change_card,
	},
	todo_check = {
		kind = "change",
		description = "Checks one of an issue's done-when criteria, by its number from 1; done false unchecks it.",
		parameters = object({ id = ID, criterion = { type = "integer" }, done = { type = "boolean" } }, { "id", "criterion" }),
		call = function(args, model, env)
			local store = store_of()
			local before = snapshot(store, args.id)
			local issue = issues.check(store, heiko.now(env), args.id, args.criterion, args.done)
			local verb = args.done == false and "unmet" or "met"
			return changed(store, issue, issue.id .. ": criterion " .. args.criterion .. " " .. verb, before)
		end,
		card = change_card,
	},
	todo_link = {
		kind = "change",
		description = "Links an issue: blocks (on blocks id, which waits for it), parent (on is the epic over id), found_while (id was found while on was worked), related. A blocks link that closes a cycle is refused. remove takes a link away.",
		parameters = object({
			id = ID,
			kind = { type = "string", enum = { "blocks", "parent", "found_while", "related" } },
			on = { type = "string", description = "The other issue." },
			remove = { type = "boolean" },
		}, { "id", "kind", "on" }),
		call = function(args, model, env)
			local store = store_of()
			local before = snapshot(store, args.id)
			local issue = issues.link(store, heiko.now(env), args.id, args.kind, args.on, args.remove)
			local line = args.remove and (args.id .. ": no longer " .. args.kind .. " " .. args.on)
				or (args.kind == "blocks" and (args.on .. " blocks " .. args.id) or (args.id .. " " .. args.kind .. " " .. args.on))
			return changed(store, issue, line, before)
		end,
		card = change_card,
	},
	todo_comment = {
		kind = "change",
		description = "Adds a comment to an issue.",
		parameters = object({ id = ID, text = { type = "string" } }, { "id", "text" }),
		call = function(args, model, env)
			local store = store_of()
			local issue = issues.comment(store, heiko.now(env), args.id, args.text, env.run.id)
			bump(store)
			return { issue = brief(issue), line = "commented on " .. issue.id }
		end,
		card = change_card,
	},
	todo_close = {
		kind = "change",
		description = "Closes an issue with the reason; with reopen, opens a closed one again. Closing with criteria unmet is allowed, but the reason says why. An epic closes once its children have.",
		parameters = object({ id = ID, reason = { type = "string" }, reopen = { type = "boolean" } }, { "id" }),
		call = function(args, model, env)
			local store = store_of()
			local before = snapshot(store, args.id)
			if args.reopen then
				local result = changed(store, issues.reopen(store, heiko.now(env), args.id), "reopened " .. args.id, before)
				local note = fold(model, env, result.issue, "reopened", result.undo)
				if note then
					note.closed = nil
				end
				return result
			end
			local issue = issues.close(store, heiko.now(env), args.id, args.reason)
			local done, all = graph.checked(issue)
			local line = "closed " .. issue.id
			if done < all then
				line = line .. ", " .. (all - done) .. " criteria unmet"
			end
			local result = changed(store, issue, line, before)
			local note = fold(model, env, issue, "closed", result.undo)
			if note then
				note.closed = true
			end
			return result
		end,
		card = change_card,
	},
	todo_defer = {
		kind = "change",
		description = "Defers an issue, until a date (2026-11-10) when given: it is not ready until then.",
		parameters = object({ id = ID, ["until"] = { type = "string" } }, { "id" }),
		call = function(args, model, env)
			local store = store_of()
			local before = snapshot(store, args.id)
			local issue = issues.defer(store, heiko.now(env), args.id, args["until"])
			local line = "deferred " .. issue.id .. (issue.defer_until and (" until " .. string.sub(time.iso(issue.defer_until), 1, 10)) or "")
			local result = changed(store, issue, line, before)
			fold(model, env, issue, "deferred", result.undo)
			return result
		end,
		card = change_card,
	},
	todo_dispatch = {
		kind = "people",
		description = "Asks the person to start a chat on an issue, with it claimed, or to send it to a chat that exists (to, its run id). It is the one thing you cannot do alone: the person allows it, or says why not.",
		parameters = object({ id = ID, to = { type = "string", description = "The chat's run id; none starts a new chat." } }, { "id" }),
		call = function(args, model, env)
			local store = store_of()
			local issue = issues.find(issues.all(store), args.id)
			if issue.claim and issue.claim ~= env.run.id then
				error(issue.id .. " is held by the run " .. issue.claim, 0)
			end
			local where = args.to and ("the chat " .. args.to) or "a new chat"
			return heiko.allow({
				title = (args.to and "Send " or "Start a chat on ") .. issue.id .. (args.to and (" to " .. args.to) or ""),
				text = "**" .. issue.title .. "**, in " .. where .. ", with it claimed there.",
				action = "dispatch",
				args = { issue = issue.id, to = args.to, from = env.run.id },
			})
		end,
	},
	todo_steps = {
		kind = "change",
		description = "Replace this run's steps with a new list: your own checklist of how you mean to do what you are doing. Each step has its text and a status, pending, in_progress or done; at most one is in progress. Send the whole list each time.",
		parameters = object({
			steps = {
				type = "array",
				items = object({
					text = { type = "string" },
					status = { type = "string", enum = { "pending", "in_progress", "done" } },
				}, { "text", "status" }),
			},
		}, { "steps" }),
		call = function(args, model, env)
			local list, why = steps.check(args.steps)
			if list == nil then
				error(why, 0)
			end
			-- A promoted step keeps the issue it became.
			for at, step in ipairs(list) do
				local was = (model.steps or {})[at]
				if was and was.issue and was.text == step.text then
					step.issue = was.issue
				end
			end
			model.steps = list
			model.step_calls = model.step_calls or {}
			table.insert(model.step_calls, env.run.call)
			-- A list all done leaves its line where this call is; sent
			-- again, the line moves here.
			if steps.done(list) == #list and #list > 0 then
				model.finished = model.finished or {}
				local last = model.finished[#model.finished]
				local same = last ~= nil and #last.steps == #list
				for at, step in ipairs(list) do
					same = same and last.steps[at].text == step.text
				end
				if same then
					table.remove(model.finished)
				end
				table.insert(model.finished, { call = env.run.call, steps = list })
			end
			return { steps = #list, done = steps.done(list) }
		end,
	},
}

-- What the board's columns are, and which an issue is in at `at`.
local COLUMNS = {
	{ id = "ready", title = "Ready" },
	{ id = "in_progress", title = "In progress" },
	{ id = "blocked", title = "Blocked" },
	{ id = "done", title = "Done" },
}

local function column_of(all, id, at)
	local issue = all[id]
	if issue.status == "closed" then
		return "done"
	end
	if issue.status == "in_progress" then
		return "in_progress"
	end
	if graph.is_blocked(all, id, at) then
		return "blocked"
	end
	if graph.is_ready(all, id, at) then
		return "ready"
	end
	return nil -- deferred: shown only through a filter
end

local function day_start(at)
	return at - at % 86400
end

-- The filter a page's params name: `label`, `type`, `priority`.
local function page_filter(params)
	return {
		label = params.label,
		type = params.type,
		priority = params.priority and tonumber(params.priority) or nil,
	}
end

local function issue_link(issue, params, label)
	local to = table.clone(params)
	to.issue = issue.id
	to.repo = nil
	to.page = nil
	return ui.link(label or issue.id, "board", { params = to })
end

-- One card of the board.
local function card(all, issue, column, params)
	local head = ui.row({
		priority(issue),
		issue_link(issue, params),
		ui.toned(issue.type, "neutral", true),
	})
	local lines = { head, ui.text(issue.title) }
	if #(issue.labels or {}) > 0 then
		table.insert(lines, ui.mono(table.concat(issue.labels, " · ")))
	end
	if column == "in_progress" and issue.claim then
		table.insert(lines, ui.toned("run " .. issue.claim, "info"))
	elseif column == "blocked" then
		local waits = graph.blockers(all, issue.id)
		table.insert(lines, ui.toned("waits on " .. (#waits > 0 and table.concat(waits, ", ") or (issue.blocked or "")), "warn"))
	elseif column == "done" and issue.close_reason then
		table.insert(lines, ui.toned(issue.close_reason, "good"))
	end
	local done, total = graph.checked(issue)
	if total > 0 then
		table.insert(lines, ui.row({ ui.ticks(done, total, "good"), ui.mono(done .. "/" .. total) }))
	end
	if column == "ready" then
		table.insert(lines, ui.button("start a run", "start", { issue = issue.id }, "secondary"))
	end
	return { id = issue.id, children = lines }
end

-- The links that switch the page's view and filters.
local function header(params, ready)
	local function to(changes)
		local next = table.clone(params)
		next.repo = nil
		next.page = nil
		next.issue = nil
		for key, value in pairs(changes) do
			next[key] = value ~= false and value or nil
		end
		return next
	end
	local view = params.view or "board"
	local links = {}
	for _, name in ipairs({ "board", "list", "graph", "beads" }) do
		local label = name:sub(1, 1):upper() .. name:sub(2)
		if name == view then
			table.insert(links, ui.rich("**" .. label .. "**"))
		else
			table.insert(links, ui.link(label, "board", { params = to({ view = name }) }))
		end
	end
	table.insert(links, ui.text("·"))
	for _, chip in ipairs({ { "p0", { priority = "0" } }, { "bugs", { type = "bug" } }, { "all done", { done = "all" } } }) do
		local on = true
		for key, value in pairs(chip[2]) do
			if params[key] ~= value then
				on = false
			end
		end
		local changes = {}
		for key, value in pairs(chip[2]) do
			changes[key] = on and false or value
		end
		table.insert(links, ui.link((on and "✓ " or "") .. chip[1], "board", { params = to(changes) }))
	end
	if params.label then
		table.insert(links, ui.link("✓ label:" .. params.label, "board", { params = to({ label = false }) }))
	end
	return ui.row({ ui.title("Todo"), ui.stamp(ready .. " ready", "good"), ui.spacer(), table.unpack(links) })
end

local function board_view(all, at, params)
	local filter = page_filter(params)
	local columns = {}
	for _, column in ipairs(COLUMNS) do
		columns[column.id] = {}
	end
	for id, issue in pairs(all) do
		local column = column_of(all, id, at)
		if params.deferred == "all" and issue.status == "deferred" and column == nil then
			column = "ready"
		end
		if column == "done" and params.done ~= "all" and (issue.closed or 0) < day_start(at) then
			column = nil
		end
		if column and graph.matches(issue, filter) then
			table.insert(columns[column], id)
		end
	end
	local out = {}
	for _, column in ipairs(COLUMNS) do
		local cards = {}
		for _, id in ipairs(graph.by_priority(all, columns[column.id])) do
			table.insert(cards, card(all, all[id], column.id, params))
		end
		table.insert(out, { id = column.id, title = column.title, cards = cards })
	end
	return ui.board(out, "move")
end

-- A section that does not fold, onto `out`: its head, with `meta` at
-- its end, then `children`.
local function headed(out, title, children, meta)
	table.insert(out, ui.header(title, { meta = meta }))
	for _, child in ipairs(children) do
		table.insert(out, child)
	end
end

-- An issue's panel: its run, description, criteria, links and actions.
local function panel(all, issue, at)
	local out = {
		ui.row({ priority(issue), ui.mono(issue.id), ui.toned(issue.type, "neutral", true), ui.stamp(issue.status, "neutral") }),
		ui.heading(issue.title),
	}
	if issue.claim then
		table.insert(out, ui.toned("held by the run " .. issue.claim, "info"))
	end
	if (issue.description or "") ~= "" then
		table.insert(out, ui.markdown(issue.description))
	end
	if #(issue.criteria or {}) > 0 then
		local rows = {}
		for n, criterion in ipairs(issue.criteria) do
			table.insert(rows, ui.row({
				check(criterion.done),
				ui.text(criterion.text),
				ui.spacer(),
				ui.button(criterion.done and "undo" or "done", "check", { id = issue.id, at = n, done = not criterion.done }),
			}))
		end
		headed(out, "Done when", rows)
	end
	local linked = {}
	for _, link in ipairs(issue.links or {}) do
		if all[link.id] then
			table.insert(linked, ui.text(link.kind .. " " .. link.id .. ": " .. all[link.id].title))
		end
	end
	for _, id in ipairs(graph.blocking(all, issue.id)) do
		table.insert(linked, ui.text("blocks " .. id .. ": " .. all[id].title))
	end
	if #linked > 0 then
		headed(out, "Links", linked)
	end
	local comments = {}
	for _, comment in ipairs(issue.comments or {}) do
		table.insert(comments, ui.rich("**" .. comment.by .. "** · " .. string.sub(time.iso(comment.at), 1, 16) .. "\n" .. comment.text))
	end
	table.insert(comments, ui.input("comment", {
		placeholder = issue.claim and "Comment, or steer the run that holds it" or "Comment",
		submit = "comment",
	}))
	headed(out, "Comments", comments)
	local actions = {}
	if issue.status == "closed" then
		table.insert(actions, ui.button("reopen", "reopen", { id = issue.id }))
	else
		if not issue.claim then
			table.insert(actions, ui.button("Start a run", "start", { issue = issue.id }, "primary"))
		end
		table.insert(actions, ui.button("close", "ask", { kind = "close", issue = issue.id }))
		table.insert(actions, ui.button("defer", "defer", { id = issue.id }))
		if issue.claim then
			table.insert(actions, ui.button("let go", "let_go", { id = issue.id }))
		end
	end
	table.insert(out, ui.row(actions))
	return ui.stack(out)
end

local GROUPS = {
	{ "in_progress", "Working now" },
	{ "ready", "Ready" },
	{ "blocked", "Blocked" },
	{ "done", "Done" },
}

local function list_view(all, at, params)
	local filter = page_filter(params)
	local out = {}
	for _, group in ipairs(GROUPS) do
		local ids = {}
		for id, issue in pairs(all) do
			if column_of(all, id, at) == group[1] and graph.matches(issue, filter) then
				if group[1] ~= "done" or params.done == "all" or (issue.closed or 0) >= day_start(at) then
					table.insert(ids, id)
				end
			end
		end
		local rows = {}
		for _, id in ipairs(graph.by_priority(all, ids)) do
			local issue = all[id]
			table.insert(rows, { issue_link(issue, params), "P" .. issue.priority, issue.title, issue.claim or "" })
		end
		if #rows > 0 then
			headed(out, group[2], { ui.table({ "Id", "P", "Title", "Run" }, rows) }, tostring(#rows))
		end
	end
	if #out == 0 then
		return ui.empty("No issues here yet: runs file them with todo_create.")
	end
	return ui.stack(out)
end

-- An epic's issues as a map: blockers left of what they block, the
-- longest way to its last open issue in the accent.
local function graph_view(all, at, params)
	local epics = {}
	for id, issue in pairs(all) do
		if issue.type == "epic" then
			table.insert(epics, id)
		end
	end
	table.sort(epics)
	if #epics == 0 then
		return ui.empty("No epics: an epic's children show here as a graph.")
	end
	local epic = params.epic or epics[1]
	if not all[epic] then
		epic = epics[1]
	end
	local picks = {}
	for _, id in ipairs(epics) do
		local to = table.clone(params)
		to.repo, to.page, to.issue = nil, nil, nil
		to.epic = id
		table.insert(picks, id == epic and ui.rich("**" .. id .. "**") or ui.link(id, "board", { params = to }))
	end
	local members = graph.children(all, epic)
	local member = {}
	for _, id in ipairs(members) do
		member[id] = true
	end
	-- Each member's depth: the longest way to it through blockers.
	local depth = {}
	local function depth_of(id, seen)
		if depth[id] then
			return depth[id]
		end
		seen = seen or {}
		if seen[id] then
			return 0
		end
		seen[id] = true
		local d = 0
		for _, by in ipairs(graph.links(all[id], "blocks")) do
			if member[by] then
				d = math.max(d, depth_of(by, seen) + 1)
			end
		end
		depth[id] = d
		return d
	end
	local deepest = 0
	for _, id in ipairs(members) do
		deepest = math.max(deepest, depth_of(id))
	end
	local rows = {}
	for _, id in ipairs(members) do
		rows[depth[id]] = rows[depth[id]] or {}
		table.insert(rows[depth[id]], id)
	end
	-- The critical way: back from the deepest open member.
	local critical = {}
	local last
	for _, id in ipairs(members) do
		if all[id].status ~= "closed" and (last == nil or depth[id] > depth[last]) then
			last = id
		end
	end
	while last do
		critical[last] = true
		local before
		for _, by in ipairs(graph.links(all[last], "blocks")) do
			if member[by] and depth[by] == depth[last] - 1 then
				before = by
			end
		end
		last = before
	end
	local nodes, edges = {}, {}
	for d = 0, deepest do
		local here = rows[d] or {}
		for n, id in ipairs(here) do
			local issue = all[id]
			table.insert(nodes, {
				id = id,
				label = id,
				x = (d + 0.5) / (deepest + 1),
				y = n / (#here + 1),
				tone = issue.status == "closed" and "good" or (critical[id] and "warn" or "neutral"),
				focus = critical[id] == true and issue.status ~= "closed",
			})
			for _, by in ipairs(graph.links(issue, "blocks")) do
				if member[by] then
					table.insert(edges, { by, id })
				end
			end
		end
	end
	local done, total = graph.progress(all, epic)
	return ui.stack({
		ui.row(picks),
		ui.row({
			ui.heading(all[epic].title),
			ui.ticks(done, math.max(total, 1), "good"),
			ui.mono(done .. "/" .. total),
		}),
		ui.graph(nodes, edges, 16),
	})
end

-- What the person is asked before a move: why it closes, or what it
-- waits on.
local function asked(store, all)
	local ask = store.get("ask")
	if not ask or not all[ask.issue] then
		return nil
	end
	local issue = all[ask.issue]
	if ask.kind == "close" then
		return ui.modal("Close " .. issue.id, {
			ui.text(issue.title),
			ui.input("close-reason", { placeholder = "Why it closes", submit = "close" }),
		}, { dismiss = "cancel" })
	end
	return ui.modal(issue.id .. " waits on", {
		ui.text(issue.title),
		ui.input("block-on", { placeholder = "An issue's id, or what outside heiko", submit = "block" }),
	}, { dismiss = "cancel" })
end

local function board_page(params, model, env)
	local store = STORE
	local ok, all = pcall(issues.all, store)
	if not ok then
		return ui.empty("Issues need the plugin's store.")
	end
	local at = heiko.now(env)
	local ready = #graph.ready(all, at)
	local view = params.view or "board"
	local body
	if view == "beads" then
		body = ui.stack({
			ui.heading("Export"),
			ui.code("json", beads.export(all)),
			ui.heading("Import"),
			ui.input("import", { placeholder = "beads' JSONL", multiline = true, submit = "import" }),
		})
	elseif view == "list" then
		body = list_view(all, at, params)
	elseif view == "graph" then
		body = graph_view(all, at, params)
	else
		body = board_view(all, at, params)
	end
	local parts = { header(params, ready) }
	if params.issue and all[params.issue] then
		local back = table.clone(params)
		back.issue, back.repo, back.page = nil, nil, nil
		table.insert(parts, ui.link("back", "board", { params = back }))
		table.insert(parts, panel(all, all[params.issue], at))
	else
		table.insert(parts, body)
	end
	local modal = asked(store, all)
	if modal then
		table.insert(parts, modal)
	end
	return ui.stack(parts)
end

-- What the person's move of a card stands for.
local function move(args, model, env)
	local store = store_of()
	local at = heiko.now(env)
	local id, to = args.card, args.to
	local all = issues.all(store)
	local issue = issues.find(all, id)
	-- Moved on another device first: this move is refused, not lost.
	local now_in = column_of(all, id, at)
	if args.from and now_in ~= args.from then
		error(id .. " is in " .. (now_in or "no column") .. " now: it moved elsewhere first", 0)
	end
	if to == now_in then
		return nil
	end
	if to == "in_progress" then
		return start_chat(store, at, id)
	elseif to == "done" then
		store.put("ask", { kind = "close", issue = id })
	elseif to == "blocked" then
		store.put("ask", { kind = "block", issue = id })
	elseif to == "ready" then
		if issue.status == "closed" then
			issues.reopen(store, at, id)
		elseif issue.claim then
			issues.let_go(store, at, id, issue.claim, "The person moved it back to ready; the run goes on without it.", "the person")
		elseif issue.status == "blocked" or issue.status == "deferred" then
			issues.update(store, at, { id = id, status = "open" })
		elseif #graph.blockers(all, id) > 0 then
			error(id .. " waits on " .. table.concat(graph.blockers(all, id), ", "), 0)
		end
	end
	bump(store)
	return nil
end

local actions = {
	move = move,
	start = function(args, model, env)
		return start_chat(store_of(), heiko.now(env), args.issue)
	end,
	-- A chat started on `issue`, or why it did not: it holds the issue.
	started = function(args, model, env)
		local store = store_of()
		local at = heiko.now(env)
		if args.error then
			issues.let_go(store, at, args.issue, "starting", nil)
			bump(store)
			return args.issue .. " was not started: " .. args.error
		end
		issues.let_go(store, at, args.issue, "starting", nil)
		issues.claim(store, at, args.issue, args.run)
		bump(store)
		return "Started the chat " .. args.run .. " on " .. args.issue .. ", which holds it."
	end,
	-- What a run asked with todo_dispatch, once the person allows it.
	dispatch = function(args, model, env)
		local store = store_of()
		local issue = issues.find(issues.all(store), args.issue)
		if issue.claim and issue.claim ~= args.from and issue.claim ~= args.to then
			return issue.id .. " is held by the run " .. issue.claim .. " now: nothing was started"
		end
		if args.to then
			return heiko.steer_chat({ run = args.to, text = opening(issue), ["then"] = "sent", args = { issue = issue.id, from = args.from } })
		end
		if issue.claim == args.from then
			issues.let_go(store, heiko.now(env), issue.id, args.from, nil)
		end
		return start_chat(store, heiko.now(env), issue.id)
	end,
	sent = function(args, model, env)
		if args.error then
			return args.issue .. " was not sent: " .. args.error
		end
		local store = store_of()
		local at = heiko.now(env)
		local issue = issues.find(issues.all(store), args.issue)
		if issue.claim and issue.claim ~= args.run then
			issues.let_go(store, at, issue.id, issue.claim, "Sent to the chat " .. args.run .. ".", "the person")
		end
		issues.claim(store, at, args.issue, args.run)
		bump(store)
		return "Sent " .. args.issue .. " to the chat " .. args.run .. ", which holds it."
	end,
	ask = function(args, model, env)
		store_of().put("ask", { kind = args.kind, issue = args.issue })
		bump(STORE)
	end,
	cancel = function(args, model, env)
		store_of().delete("ask")
		bump(STORE)
	end,
	close = function(args, model, env)
		local store = store_of()
		local ask = store.get("ask")
		if not ask then
			return nil
		end
		issues.close(store, heiko.now(env), ask.issue, args.text ~= "" and args.text or "Closed from the board.")
		store.delete("ask")
		bump(store)
	end,
	block = function(args, model, env)
		local store = store_of()
		local ask = store.get("ask")
		if not ask then
			return nil
		end
		local on = string.match(args.text or "", "^%s*(td%-[%w%.]+)%s*$")
		local all = issues.all(store)
		if on and all[on] then
			issues.link(store, heiko.now(env), ask.issue, "blocks", on)
			local issue = issues.all(store)[ask.issue]
			if issue.claim then
				issues.let_go(store, heiko.now(env), issue.id, issue.claim, "Blocked on " .. on .. ".", "the person")
			end
		else
			issues.update(store, heiko.now(env), { id = ask.issue, status = "blocked", blocked = args.text })
		end
		store.delete("ask")
		bump(store)
	end,
	reopen = function(args, model, env)
		issues.reopen(store_of(), heiko.now(env), args.id)
		bump(STORE)
	end,
	defer = function(args, model, env)
		issues.defer(store_of(), heiko.now(env), args.id, args["until"])
		bump(STORE)
	end,
	let_go = function(args, model, env)
		local store = store_of()
		local issue = issues.find(issues.all(store), args.id)
		if issue.claim then
			issues.let_go(store, heiko.now(env), issue.id, issue.claim, "The person let it go; the run goes on without it.", "the person")
		end
		bump(store)
	end,
	check = function(args, model, env)
		issues.check(store_of(), heiko.now(env), args.id, args.at, args.done)
		bump(STORE)
	end,
	-- A comment on the issue the panel shows; the run that holds it is
	-- steered with it too.
	comment = function(args, model, env)
		local store = store_of()
		local id = args.issue or (args.params and args.params.issue)
		if not id then
			return nil
		end
		local issue = issues.comment(store, heiko.now(env), id, args.text, "the person")
		bump(store)
		if issue.claim and issue.claim ~= "starting" then
			return heiko.steer_chat({ run = issue.claim, text = "On " .. issue.id .. ": " .. args.text })
		end
	end,
	-- Undoes a change a run's card shows.
	undo = function(args, model, env)
		local store = store_of()
		-- The issue's note says so, and has nothing more to undo.
		local was = args.restore
		local note = note_of(model, args.forget or (was and was.id))
		if note then
			note.undo = nil
			table.insert(note.said, "undone")
			if was then
				note.title = was.title
				note.closed = was.status == "closed" or nil
			end
		end
		if args.forget then
			issues.delete(store, heiko.now(env), args.forget)
		elseif args.restore then
			local issue = args.restore
			-- A claim comes back only with its run going on: an ended run's
			-- would hold the issue with nothing to let it go, so the issue
			-- goes back to open, as an ended run's does.
			if issue.claim ~= nil and not (issue.claim == env.run.id and env.run.live) then
				issue = table.clone(issue)
				issue.claim = nil
				if issue.status == "in_progress" then
					issue.status = "open"
				end
			end
			store.put("issue/" .. issue.id, issue)
		end
		bump(store)
	end,
	-- A step of the run's becomes an issue, found while its issue.
	promote = function(args, model, env)
		local store = store_of()
		local step = (model.steps or {})[tonumber(args.at) or 0]
		if not step then
			return nil
		end
		local issue = issues.create(store, heiko.now(env), { title = step.text, found_while = held(store, env.run.id) })
		-- The steps as they are now: promoted only if it is still that step.
		local still = (model.steps or {})[tonumber(args.at) or 0]
		if still and still.text == step.text then
			promote(model, args.at, issue.id)
		end
		bump(store)
	end,
	-- Beads' JSONL the person pasted, into this repository.
	import = function(args, model, env)
		local store = store_of()
		local n = issues.import(store, heiko.now(env), beads.import(args.text or ""))
		bump(store)
		return "Imported " .. n .. " issues."
	end,
}

-- The band's lines for the run's steps.
local function step_lines(list)
	local lines = {}
	for at, step in ipairs(list) do
		local line = step.text
		if step.issue then
			line = line .. " → " .. step.issue
		end
		local current = step.status == "in_progress"
		local row = { check(step.status == "done", tostring(at), current), current and ui.text(line) or ui.toned(line, "neutral") }
		if step.status ~= "done" and not step.issue then
			table.insert(row, ui.spacer())
			table.insert(row, ui.button("make an issue", "promote", { at = at }))
		end
		table.insert(lines, ui.row(row))
	end
	return lines
end

-- What the band shows of the issue the run holds, kept in the model as
-- a hook leaves the store: the view draws from it, and waits on nothing.
local function refresh(model, env)
	local run = env.run and env.run.id
	if run == nil then
		return
	end
	local all = issues.all(STORE)
	local id = nil
	for other, issue in pairs(all) do
		if issue.claim == run and (id == nil or other < id) then
			id = other
		end
	end
	if not id then
		model.held = nil
		return
	end
	local issue = all[id]
	local found = {}
	for other, candidate in pairs(all) do
		for _, link in ipairs(candidate.links or {}) do
			if link.kind == "found_while" and link.id == id then
				table.insert(found, other)
			end
		end
	end
	table.sort(found)
	local ready = graph.ready(all, heiko.now(env))
	local next = {}
	for n = 1, math.min(3, #ready) do
		table.insert(next, ready[n])
	end
	local criteria = {}
	for _, criterion in ipairs(issue.criteria or {}) do
		table.insert(criteria, { text = criterion.text, done = criterion.done == true })
	end
	model.held = { id = id, title = issue.title, criteria = criteria, found = found, next = next }
end

-- `f` as a hook that leaves the band's issue as the store has it.
local function refreshing(f)
	local whole = atomic(f)
	return function(input, model, env)
		local out = whole(input, model, env)
		-- What it wrote goes in as this reads: the step ends here.
		refresh(model, env)
		return out
	end
end

-- The band over the composer, one line that opens: the step the run is
-- on while any is left, or else the issue it holds. Opened, the issue's
-- criteria and the steps.
local function band(model)
	local list = model.steps or {}
	local held_issue = model.held
	local id = held_issue and held_issue.id
	local left = #list > 0 and steps.done(list) < #list
	local lead, lines = {}, {}
	if left then
		local inks, on = {}, nil
		for at, step in ipairs(list) do
			if step.status == "in_progress" then
				on = at
			end
			table.insert(inks, step.status == "done" and "good" or step.status == "in_progress" and "warn" or "neutral")
		end
		table.insert(lead, ui.pips(inks))
		table.insert(lead, ui.toned((on or steps.done(list)) .. " of " .. #list, "warn", true))
		table.insert(lead, on and ui.text(list[on].text) or ui.toned(held_issue and held_issue.title or "Steps", "neutral"))
	end
	if held_issue then
		local done, total = 0, #held_issue.criteria
		for _, criterion in ipairs(held_issue.criteria) do
			if criterion.done then
				done += 1
			end
		end
		local name = { ui.toned(id, "good", true), ui.text(held_issue.title) }
		if total > 0 then
			table.insert(name, ui.toned(done .. "/" .. total, "neutral", true))
		end
		if left then
			table.insert(lines, ui.row(name))
		else
			lead = name
		end
		for _, criterion in ipairs(held_issue.criteria) do
			table.insert(lines, ui.row({ check(criterion.done), ui.toned(criterion.text, criterion.done and "good" or "neutral") }))
		end
		if #held_issue.found > 0 then
			table.insert(lines, ui.mono("found: " .. table.concat(held_issue.found, ", ")))
		end
		if #held_issue.next > 0 then
			table.insert(lines, ui.mono("next ready: " .. table.concat(held_issue.next, ", ")))
		end
	end
	if left then
		for _, line in ipairs(step_lines(list)) do
			table.insert(lines, line)
		end
	end
	if #lead == 0 then
		return nil, nil
	end
	if #lines == 0 then
		return ui.row(lead), id
	end
	return ui.section("todo-band", "", lines, { small = true, lead = lead }), id
end

for _, tool in pairs(tools) do
	tool.call = refreshing(tool.call)
end
for name, action in pairs(actions) do
	actions[name] = refreshing(action)
end

-- How many ready issues Home lists.
local HOME_ROWS = 3

-- What Home lists of the repository: the first ready issues, each
-- putting what a run on it starts with in the new run's composer.
local function home(all, ready)
	local rows = array({})
	for n = 1, math.min(HOME_ROWS, #ready) do
		local issue = all[ready[n]]
		table.insert(rows, {
			id = issue.id,
			title = issue.title,
			detail = "P" .. issue.priority,
			write = "Work on " .. issue.id .. ": " .. issue.title .. ".",
		})
	end
	return { title = "Ready to pick up", meta = "this repository's issues", rows = rows }
end

return heiko.plugin({
	uses = { store = true },
	tools = tools,
	actions = actions,
	pages = { board = board_page },
	repo_nav = { label = "Todo", page = "board", icon = "list" },
	repo_data = function(_, model, env)
		local ok, all = pcall(issues.all, STORE)
		if not ok then
			return {}
		end
		local ready = graph.ready(all, heiko.now(env))
		return {
			rev = STORE.get("rev") or 0,
			ready = #ready,
			nav = { badge = #ready > 0 and tostring(#ready) or nil, detail = #ready .. " ready" },
			home = home(all, ready),
		}
	end,
	start = function(plan, model, env)
		refresh(model, env)
		return heiko.start({ instructions = PROMPT })
	end,
	-- What another run or the board changed shows in the band by the
	-- next turn.
	turn_end = function(_, model, env)
		refresh(model, env)
	end,
	-- A claim is its run's: as the run ends, what it holds goes back to
	-- open, with what it did and the steps it left.
	run_end = atomic(function(run, model, env)
		local held = issues.held_by(STORE, env.run.id)
		-- The steps as they are now, after the read.
		local left = {}
		for _, step in ipairs(model.steps or {}) do
			if step.status ~= "done" then
				table.insert(left, "- " .. step.text)
			end
		end
		local said = run.text and string.sub(run.text, 1, 600) or ""
		local note = "The run " .. env.run.id .. " ended (" .. tostring(run.stop) .. ")."
		if said ~= "" then
			note = note .. " It said: " .. said
		end
		if #left > 0 then
			note = note .. "\n\nSteps it left:\n" .. table.concat(left, "\n")
		end
		for _, id in ipairs(held) do
			issues.let_go(STORE, heiko.now(env), id, env.run.id, note)
		end
		if #held > 0 then
			bump(STORE)
		end
		model.held = nil
	end),
	-- As heiko starts: what the runs it found cut off held goes back to
	-- open, and no chat is being started any more.
	recover = atomic(function(input, model, env)
		local any = false
		for _, run in ipairs(input.runs or {}) do
			for _, id in ipairs(issues.held_by(STORE, run)) do
				issues.let_go(STORE, heiko.now(env), id, run, "heiko closed while the run " .. run .. " was going.", "heiko")
				any = true
			end
		end
		for _, id in ipairs(issues.held_by(STORE, "starting")) do
			issues.let_go(STORE, heiko.now(env), id, "starting", nil)
			any = true
		end
		if any then
			bump(STORE)
		end
	end),
	view = function(model, env)
		local drawn, id = band(model)
		return { band = drawn, row = id and { line = id } or nil, calls = folded(model) }
	end,
})
