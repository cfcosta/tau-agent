-- Checks a run's tool calls and final answer against the repository's
-- rules, asking Jev how likely each rule is broken (ADR 0050;
-- docs/reference/constitution.md). Before a call, the rules that name
-- one of its arguments are asked about in one request: Jev sees only
-- those fields, as the model wrote them. At or past a rule's block mark
-- the call is refused with the rule, between review and block it runs,
-- flagged for a person. Before the run stops, the rules on the final
-- answer are checked the same way, and a broken one sends the answer
-- back, up to `max_holds` times. When Jev cannot answer, `on_error`
-- decides. The rules, the review queue and what the checks did live in
-- the repository's store; the page edits them, and an edit applies from
-- the next tool call.
local tau = require("tau")
local ui = tau.ui
local rules = require("rules")
local checks = require("checks")
local page = require("page")

local cmd = tau.cmd
local NAME = checks.NAME

-- The repository's store, as commands: a read awaits it, a write is
-- given and lands when the step ends, before the next read.
local store = {
	get = function(key)
		return tau.await(cmd.store.get(key))
	end,
	list = function(prefix)
		return tau.await(cmd.store.list(prefix))
	end,
	put = function(key, value)
		tau.give(cmd.store.put(key, value))
	end,
	delete = function(key)
		tau.give(cmd.store.delete(key))
	end,
}

-- Whether two values read from the store hold the same.
local function same(a, b)
	if type(a) ~= "table" or type(b) ~= "table" then
		return a == b
	end
	for key, value in pairs(a) do
		if not same(value, b[key]) then
			return false
		end
	end
	for key in pairs(b) do
		if a[key] == nil then
			return false
		end
	end
	return true
end

-- How many past calls, and answers, the store keeps for Try it.
local SAMPLE_CALLS = 24
local SAMPLE_ANSWERS = 3

-- A call's arguments past this, as JSON, are not kept for Try it.
local SAMPLE_BYTES = 64 * 1024

-- How many of what the rules handled the page lists.
local HANDLED = 30

-- How long a run's title, its first message's first line, may be.
local TITLE_CHARS = 60

-- The repository's constitution, checked; none stored is no rules.
-- Else nil and why.
local function constitution_of()
	return rules.from_stored(store.get("constitution"))
end

-- `constitution` as the store keeps it.
local function stored(constitution)
	return { on_error = constitution.on_error, max_holds = constitution.max_holds, rules = constitution.rules }
end

-- Tells the page and the sidebar that what they draw changed.
local function bump()
	store.put("rev", (store.get("rev") or 0) + 1)
end

-- Puts `entry` first in `list`, unless it is there already, keeping at
-- most `most`.
local function newest(list, entry, most)
	local key = json.encode(entry)
	local kept = array({ entry })
	for _, known in ipairs(list or {}) do
		if #kept < most and json.encode(known) ~= key then
			table.insert(kept, known)
		end
	end
	return kept
end

-- Keeps a call for Try it, when a rule, or a place the editor offers,
-- reads its tool.
local function sample_call(constitution, call)
	local tools = constitution and rules.tools(constitution) or {}
	for _, place in ipairs(page.PLACES) do
		local target = rules.target(place[1])
		if not target.final then
			tools[target.tool] = true
		end
	end
	if not tools[call.name] or #json.encode(call.args) > SAMPLE_BYTES then
		return
	end
	local samples = store.get("samples") or {}
	samples.calls = newest(samples.calls, { tool = call.name, args = call.args }, SAMPLE_CALLS)
	store.put("samples", samples)
end

local function sample_answer(answer)
	if answer == "" then
		return
	end
	local samples = store.get("samples") or {}
	samples.answers = newest(samples.answers, answer, SAMPLE_ANSWERS)
	store.put("samples", samples)
end

-- Something the rules handled, for the page's list.
local function handled(entry)
	local list = array({})
	for _, known in ipairs(store.get("handled") or {}) do
		table.insert(list, known)
	end
	table.insert(list, entry)
	while #list > HANDLED do
		table.remove(list, 1)
	end
	store.put("handled", list)
end

-- A flagged call or answer, for a person to look at.
local function waiting(item)
	local seq = (store.get("seq") or 0) + 1
	store.put("seq", seq)
	item.id = string.format("review/%09d", seq)
	store.put(item.id, item)
end

-- What rule `rule` last caught, for its caption.
local function caught(rule, shown)
	local last = store.get("caught") or {}
	last[rule] = checks.cut(string.match(shown, "^[^\n]*"), TITLE_CHARS)
	store.put("caught", last)
end

-- Folds a check's `records` into the run's state and keeps what the
-- repository's page shows of them: the run's counts, what waits for a
-- person, and what the rules handled. `call` is the call checked (nil
-- for the final answer), `shown` what its rules read, and `answer` the
-- final answer. The run's state is folded first, in one step; the store's
-- reads after it may wait, and its last write reads the state again.
local function apply(model, env, records, call, shown, answer)
	local state = checks.state(model)
	for _, record in ipairs(records) do
		checks.fold(state, record, answer, shown)
	end
	local title = state.title or env.run.id
	if call ~= nil then
		local verdict = (state.calls[call.id] or {}).verdict
		if verdict and verdict.kind == "blocked" then
			handled({ what = "blocked", shown = call.name .. " " .. shown, rule = verdict.rule, title = title })
		elseif verdict then
			waiting({
				run = env.run.id,
				tool = call.name,
				shown = shown,
				rule = verdict.rule,
				score = verdict.score,
				title = title,
			})
		end
		if verdict then
			caught(verdict.rule, shown)
		end
	end
	for _, record in ipairs(records) do
		if record.kind == "held" then
			handled({ what = "held", shown = "final answer", rule = record.rule, title = title })
			caught(record.rule, "final answer, " .. title)
		elseif record.kind == "flagged" and record.call_id == nil then
			waiting({
				run = env.run.id,
				shown = checks.cut(answer or "", checks.SHOWN_CHARS),
				rule = record.rule,
				score = record.score,
				title = title,
			})
		end
	end
	-- The state as it is now, after the store's reads.
	state = checks.state(model)
	store.put("run/" .. env.run.id, { title = state.title or env.run.id, stats = state.stats })
	bump()
end

-- The fields every rule about a call reads, as one text.
local function shown_of(about)
	local fields, names = {}, {}
	for _, found in ipairs(about) do
		for _, name in ipairs(found.names) do
			if fields[name] == nil then
				table.insert(names, name)
			end
			fields[name] = found.fields[name]
		end
	end
	return checks.cut(checks.shown(fields, names), checks.SHOWN_CHARS)
end

-- One of its notes: a check Jev could not answer, an answer held or
-- flagged.
local function note_tree(note)
	return ui.row({ ui.icon("blocked", note.tone), ui.mono(NAME), ui.text(note.text), ui.mono(note.detail) })
end

-- The rule `rule` on the repository's page.
local function rule_link(label, repo, rule)
	return ui.link(label, "rules", { params = { repo = repo or "", rule = rule } })
end

-- What the checks draw on a call's card: beside its title, the rule
-- that flagged it and its score with a way to review it, or quietly the
-- scores that passed; under a blocked one, the rule and Jev's score
-- against its marks.
local function call_tree(call, repo)
	local verdict = call.verdict
	if verdict and verdict.kind == "flagged" then
		return {
			badge = ui.row({
				ui.mono(verdict.rule .. " " .. checks.two(verdict.score)),
				rule_link("Review", repo, verdict.rule),
			}),
			mark = "flagged",
		}
	elseif verdict then
		local meter = "p(violation) " .. checks.two(verdict.score)
		if verdict.review and verdict.block then
			meter ..= " · review " .. checks.two(verdict.review) .. " · block " .. checks.two(verdict.block)
		end
		return {
			body = ui.row({
				ui.badge(NAME, "bad"),
				rule_link("Rule " .. verdict.rule, repo, verdict.rule),
				ui.progress(verdict.score, meter),
			}),
			mark = "blocked",
			reason = verdict.reason,
		}
	end
	if #call.scores == 0 then
		return nil
	end
	local scores = {}
	for _, score in ipairs(call.scores) do
		table.insert(scores, score.rule .. " " .. checks.two(score.score))
	end
	return { badge = ui.mono(table.concat(scores, " · ")) }
end

-- The inspector's section: what the checks did, and a flagged call to
-- review once the run is done; nothing before anything is checked.
local function inspector(state)
	local stats = state.stats
	if checks.is_empty(stats) then
		return nil
	end
	local function with_rules(list)
		if #list == 0 then
			return "0"
		end
		return #list .. " (" .. table.concat(list, ", ") .. ")"
	end
	local held = with_rules(stats.held)
	if stats.max_holds and #stats.held > 0 then
		held = #stats.held .. " of " .. stats.max_holds .. " (" .. table.concat(stats.held, ", ") .. ")"
	end
	local section = {
		ui.heading("Constitution checks"),
		ui.rows({
			{ "calls checked", stats.calls },
			{ "answers checked", stats.answers },
			{ "questions asked", stats.questions },
			{ "blocked", with_rules(stats.blocked) },
			{ "flagged", with_rules(stats.flagged) },
			{ "held", held },
			{ "Jev cost", checks.usd(stats.cost) },
		}),
		ui.text(
			"Only the fields the model wrote are checked, such as edit.newText or exec_command.cmd. Tool output never goes to Jev."
		),
	}
	if #stats.flagged > 0 and not state.live then
		table.insert(section, rule_link("Review the flagged call", state.repo, stats.flagged[1]))
	end
	return ui.stack(section)
end

-- The repository's page: its rules, the editor, and what waits for
-- review, from its store.
local function rules_page(params, _, env)
	local repo = params.repo
	local data = {
		repo = repo,
		jev = env.run.jev == true,
		focus = params.rule,
		editor = store.get("editor"),
		trying = store.get("trying"),
		resetting = store.get("resetting") == true,
		handled = store.get("handled") or {},
		caught = store.get("caught") or {},
		waiting = {},
	}
	local constitution, why = constitution_of()
	if constitution then
		data.constitution = constitution
	else
		data.error = rules.errors.invalid(repo, why)
	end
	local runs = {}
	for _, entry in ipairs(store.list("run/")) do
		table.insert(runs, entry.value.stats)
	end
	data.totals = checks.totals(runs)
	for _, entry in ipairs(store.list("review/")) do
		table.insert(data.waiting, entry.value)
	end
	return page.tree(data)
end

-- Changes the repository's constitution with `edit`, which checks what
-- it changes, and saves it; `failed` says what could not be done.
local function edit(env, failed, change)
	local constitution, why = constitution_of()
	if constitution == nil then
		error(failed .. ": " .. rules.errors.invalid(env.run.repo or "", why), 0)
	end
	local ok, refused = change(constitution)
	if ok == nil then
		error(failed .. ": " .. refused, 0)
	end
	store.put("constitution", stored(constitution))
	bump()
end

-- Changes the rule being written with `change`; `fresh` drops what
-- trying it gave.
local function drafting(fresh, change)
	local draft = store.get("editor")
	if draft == nil then
		return
	end
	change(draft)
	store.put("editor", draft)
	if fresh then
		store.delete("trying")
	end
	bump()
end

local actions = {
	-- Opens the editor on a new rule, or on `rule`.
	open = function(args, _, env)
		local constitution = constitution_of()
		local rule = constitution and args.rule and rules.find(constitution, args.rule)
		store.put("editor", page.draft(rule))
		store.delete("trying")
		bump()
	end,
	close = function(_, _, env)
		store.delete("editor")
		store.delete("trying")
		bump()
	end,
	-- Enter in the rule's field: what it said is the rule.
	write = function(args, _, env)
		drafting(false, function(draft)
			if rules.trim(args.text or "") ~= "" then
				draft.text = args.text
			end
		end)
	end,
	place = function(args, _, env)
		drafting(true, function(draft)
			page.toggle_place(draft, args.place)
		end)
	end,
	-- Enter in the field's field: the field it names.
	add_place = function(args, _, env)
		drafting(true, function(draft)
			page.add_place(draft, args.text or "")
		end)
	end,
	preset = function(args, _, env)
		drafting(false, function(draft)
			local preset = page.preset_named(args.preset)
			if preset then
				draft.review, draft.block = preset.review, preset.block
			end
		end)
	end,
	nudge = function(args, _, env)
		drafting(false, function(draft)
			page.nudge(draft, args.mark, args.delta)
		end)
	end,
	-- Adds or saves the rule; with something missing, the editor says
	-- what.
	save = function(_, _, env)
		local draft = store.get("editor")
		if draft == nil then
			return
		end
		if page.problem(draft) then
			draft.tried_to_save = true
			store.put("editor", draft)
			bump()
			return
		end
		local text = rules.trim(draft.text)
		if draft.editing then
			edit(env, "Could not save the rule", function(constitution)
				return rules.replace(constitution, draft.editing, text, draft.places, draft.review, draft.block)
			end)
		else
			edit(env, "Could not add the rule", function(constitution)
				return rules.add(constitution, text, draft.places, draft.review, draft.block)
			end)
		end
		store.delete("editor")
		store.delete("trying")
	end,
	-- Tries the rule being written on the latest calls and answers it
	-- reads, with Jev.
	try = function(_, _, env)
		local draft = store.get("editor")
		if draft == nil then
			return
		end
		if page.problem(draft) then
			draft.tried_to_save = true
			store.put("editor", draft)
			bump()
			return
		end
		if not env.run.jev then
			store.put("trying", { failed = "Trying a rule asks Jev: add a TypeSafe key on the Models screen." })
		else
			local calls, answers = page.trial_samples(draft, store.get("samples") or {})
			local trials, cost = page.try(draft, calls, answers)
			-- The rule as it is now: changed or closed while Jev answered,
			-- what it gave is not this rule's.
			if not same(store.get("editor"), draft) then
				return
			end
			if trials == nil then
				store.put("trying", { failed = cost })
			else
				store.put("trying", { trials = trials, cost = cost })
			end
		end
		bump()
	end,
	remove = function(args, _, env)
		edit(env, "Could not remove the rule", function(constitution)
			rules.remove(constitution, args.id)
			return true
		end)
	end,
	-- What Jev cannot answer, and how many times an answer may go back.
	settings = function(args, _, env)
		edit(env, "Could not save the constitution", function(constitution)
			constitution.on_error = args.blocks_unchecked and "block" or "allow"
			constitution.max_holds = args.max_holds
			return true
		end)
	end,
	-- Asks to remove rules that cannot be read, to be confirmed, or
	-- takes the question back.
	ask_reset = function(args, _, env)
		if args.yes then
			store.put("resetting", true)
		else
			store.delete("resetting")
		end
		bump()
	end,
	-- Removes them, as confirmed: no rules, to start again.
	reset = function(_, _, env)
		store.put("constitution", stored(rules.default()))
		store.delete("resetting")
		bump()
	end,
	-- A flagged call or answer looked fine: off the queue, for good.
	fine = function(args, _, env)
		local item = args.id and store.get(args.id)
		if item == nil then
			return
		end
		store.delete(args.id)
		local shown = item.tool and (item.tool .. " " .. item.shown) or ("final answer: " .. item.shown)
		handled({ what = "fine", shown = shown, rule = item.rule, title = item.title })
		bump()
	end,
}

return tau.plugin({
	uses = { jev = true, store = true },
	-- The page lets an answer go back up to ten times.
	continues = page.MAX_HOLDS,

	-- As a message comes in: what the checks say before anything is
	-- checked. Rules that cannot be read fail the run, once there is a
	-- key to check them with: they are never skipped.
	start = function(plan, model, env)
		local state = checks.state(model)
		state.holds = 0
		state.live = true
		state.repo = env.run.repo
		if state.title == nil then
			state.title = checks.cut(string.match(plan.input or "", "^[^\n]*"), TITLE_CHARS)
		end
		if not env.run.jev then
			state.starting = checks.starting(false)
			return nil
		end
		local constitution, why = constitution_of()
		state = checks.state(model)
		if constitution == nil then
			state.starting = checks.starting(true, nil)
			return tau.fail(rules.errors.invalid(env.run.repo or "", why))
		end
		state.starting = checks.starting(true, #constitution.rules)
		return nil
	end,

	-- Before a call: the rules that read one of its fields, all in one
	-- request.
	before_tool = function(call, model, env)
		local constitution = constitution_of()
		sample_call(constitution, call)
		if not env.run.jev or constitution == nil then
			return nil
		end
		local about = rules.for_call(constitution, call.name, call.args)
		if #about == 0 then
			return nil
		end
		local shown = shown_of(about)
		local scores, cost = checks.ask(checks.call_request(call.name, about), #about)
		local records, reason
		if scores == nil then
			local record
			record, reason = checks.call_failed(call, cost, constitution.on_error)
			records = { record }
		else
			records, reason = checks.of_call(call, about, scores, cost)
		end
		apply(model, env, records, call, shown, nil)
		if reason then
			return tau.block(reason)
		end
		return nil
	end,

	-- Before the run stops: the rules on the final answer.
	before_stop = function(stop, model, env)
		local constitution = constitution_of()
		sample_answer(stop.text or "")
		if not env.run.jev or constitution == nil then
			return nil
		end
		local found = rules.for_final_answer(constitution)
		if #found == 0 then
			return nil
		end
		local answer = stop.text or ""
		local scores, cost = checks.ask(checks.answer_request(answer, found), #found)
		-- The holds as they are now, after Jev answered.
		local holds = model.holds or 0
		local records, message
		if scores == nil then
			local record
			record, message = checks.answer_failed(cost, constitution.on_error, holds, constitution.max_holds)
			records = { record }
		else
			records, message = checks.of_answer(found, scores, cost, holds, constitution.max_holds)
		end
		-- The hold is counted before the store's reads in `apply`, so
		-- nothing comes between reading the holds and counting this one.
		local state = checks.state(model)
		if message ~= nil then
			state.holds = holds + 1
		end
		apply(model, env, records, nil, nil, answer)
		if message == nil then
			return nil
		end
		state = checks.state(model)
		-- Its note says it: the message draws as nothing.
		state.said[message] = true
		return tau.continue(message)
	end,

	run_end = function(_, model)
		model.live = nil
	end,

	-- Of a frozen copy: what `checks.state` would fill in is read as
	-- empty, not written.
	view = function(state)
		local notes = {}
		for key, entry in pairs(state.notes or {}) do
			notes[key] = note_tree(entry)
		end
		local calls = {}
		for id, call in pairs(state.calls or {}) do
			calls[id] = call_tree(call, state.repo)
		end
		local messages = {}
		for text in pairs(state.said or {}) do
			messages[text] = ui.stack({})
		end
		local shown = { stats = state.stats or checks.stats(), starting = state.starting, live = state.live, repo = state.repo }
		local status, tone = checks.status(shown)
		return {
			notes = notes,
			calls = calls,
			messages = messages,
			status = status and ui.status(status, nil, tone),
			inspector = inspector(shown),
		}
	end,

	actions = actions,
	pages = { rules = rules_page },
	repo_nav = { label = "Constitution", page = "rules", icon = "blocked" },
	-- The sidebar's entry: how many rules, and what waits for a person.
	repo_data = function()
		local constitution = constitution_of()
		local count = constitution and #constitution.rules or 0
		local waiting = #store.list("review/")
		return {
			rev = store.get("rev") or 0,
			rules = count,
			waiting = waiting,
			nav = { detail = count .. " rules", badge = waiting > 0 and tostring(waiting) or nil, tone = "warn" },
		}
	end,
})
