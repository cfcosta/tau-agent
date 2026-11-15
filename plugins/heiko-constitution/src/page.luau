-- A repository's rules page: what each rule reads, how strict it is
-- and what it did, the calls and answers that wait for a person, and
-- the editor that writes and tries a rule. The rule being written, what
-- trying it gave and a removal waiting to be confirmed are kept in the
-- repository's store, as the rules are.
local heiko = require("heiko")
local ui = heiko.ui
local rules = require("rules")
local checks = require("checks")

local page = {}

-- The places the editor offers, with what each reads.
page.PLACES = {
	{ "exec_command.cmd", "the shell command" },
	{ "edit.newText", "each replacement" },
	{ "write.content", "the whole file" },
	{ "final answer", "sent back when broken" },
}

-- Thresholds by name, review then block. The editor offers these first.
page.PRESETS = {
	{ name = "lenient", label = "Lenient", review = 0.5, block = 0.9 },
	{ name = "balanced", label = "Balanced", review = 0.3, block = 0.8 },
	{ name = "strict", label = "Strict", review = 0.2, block = 0.6 },
}

-- How many past calls, and answers, a rule is tried on.
page.TRIAL_CALLS = 6
page.TRIAL_ANSWERS = 3

-- The most times the page lets one run's answer be sent back.
page.MAX_HOLDS = 10

-- The tabs' id: the rules, then what waits for review.
page.TABS = "rules-tabs"

-- A threshold to two places, so steps of 0.05 do not drift.
function page.round(value)
	return math.round(value * 100) / 100
end

-- How many answers may go back after a step of `delta` from `holds`:
-- from none to `MAX_HOLDS`.
function page.next_holds(holds, delta)
	return math.min(math.max(holds + delta, 0), page.MAX_HOLDS)
end

function page.preset_named(name)
	for _, preset in ipairs(page.PRESETS) do
		if preset.name == name then
			return preset
		end
	end
	return nil
end

-- A rule being written: a new one, or `rule` as it is. `text` is the
-- rule as Enter kept it from its field.
function page.draft(rule)
	local balanced = page.preset_named("balanced")
	local places = array({})
	for _, place in ipairs(rule and rule.on or {}) do
		table.insert(places, place)
	end
	return {
		editing = rule and rule.id,
		text = rule and rule.text,
		places = places,
		review = rule and page.round(rule.review) or balanced.review,
		block = rule and page.round(rule.block) or balanced.block,
		tried_to_save = false,
	}
end

-- The preset the draft's thresholds are, if any.
function page.preset(draft)
	for _, preset in ipairs(page.PRESETS) do
		if math.abs(preset.review - draft.review) < 1e-6 and math.abs(preset.block - draft.block) < 1e-6 then
			return preset
		end
	end
	return nil
end

-- Picks or unpicks a place the rule applies.
function page.toggle_place(draft, place)
	local at = table.find(draft.places, place)
	if at then
		table.remove(draft.places, at)
	else
		table.insert(draft.places, place)
	end
end

-- Adds `typed`, if it names a field (`tool.field`): whether it did.
function page.add_place(draft, typed)
	local target = rules.target(typed)
	if target == nil then
		draft.bad_place = rules.trim(typed)
		return false
	end
	draft.bad_place = nil
	local place = rules.label(target)
	if not table.find(draft.places, place) then
		table.insert(draft.places, place)
	end
	return true
end

-- Moves a threshold by `delta`, keeping review at most block.
function page.nudge(draft, mark, delta)
	if mark == "review" then
		draft.review = page.round(math.clamp(draft.review + delta, 0, draft.block))
	else
		draft.block = page.round(math.clamp(draft.block + delta, draft.review, 1))
	end
end

-- What keeps the draft from being saved, if anything.
function page.problem(draft)
	if rules.trim(draft.text or "") == "" then
		return "Write the rule: one sentence, the way you would tell a person, then Enter."
	elseif #draft.places == 0 then
		return "Pick at least one place, or Jev has nothing to check."
	end
	return nil
end

-- The latest calls and answers the draft's places read, from `samples`
-- (`{ calls = { { tool, args } }, answers = { ... } }`, newest first).
function page.trial_samples(draft, samples)
	local tools, final = {}, false
	for _, place in ipairs(draft.places) do
		local target = rules.target(place)
		if target and target.final then
			final = true
		elseif target then
			tools[target.tool] = true
		end
	end
	local calls, answers = {}, {}
	for _, call in ipairs(samples.calls or {}) do
		if tools[call.tool] and #calls < page.TRIAL_CALLS then
			table.insert(calls, call)
		end
	end
	if final then
		for _, answer in ipairs(samples.answers or {}) do
			if #answers < page.TRIAL_ANSWERS then
				table.insert(answers, answer)
			end
		end
	end
	return calls, answers
end

-- Asks Jev what a rule being written makes of past `calls` and
-- `answers`, as a check would ask about each, one request each: the
-- trials, `{ tool, shown, score }`, and what Jev cost; else nil and
-- why. Nothing runs again; Jev sees only what a check would show it.
function page.try(draft, calls, answers)
	for _, place in ipairs(draft.places) do
		local target, why = rules.target(place)
		if target == nil then
			return nil, why
		end
	end
	local rule = { id = "new", text = rules.trim(draft.text or ""), on = draft.places, review = draft.review, block = draft.block }
	local one = { rules = { rule } }
	local asks = {}
	for _, call in ipairs(calls) do
		local about = rules.for_call(one, call.tool, call.args)[1]
		if about then
			table.insert(asks, {
				tool = call.tool,
				shown = checks.shown(about.fields, about.names),
				request = checks.call_request(call.tool, { about }),
			})
		end
	end
	if #rules.for_final_answer(one) > 0 then
		for _, answer in ipairs(answers) do
			table.insert(asks, { shown = answer, request = checks.answer_request(answer, { rule }) })
		end
	end
	local trials, cost = array({}), 0
	for _, ask in ipairs(asks) do
		local scores, spent = checks.ask(ask.request, 1)
		if scores == nil then
			return nil, spent
		end
		cost += spent
		table.insert(trials, { tool = ask.tool, shown = ask.shown, score = scores[1] })
	end
	return trials, cost
end

local function button(label, action, args, kind)
	return ui.button(label, action, args or {}, kind or "secondary")
end

-- How many ticks a score's strip has.
local TICKS = 20

-- The tick a threshold or a score falls on, from 0.
local function tick(value)
	return math.clamp(math.floor(value * TICKS + 0.5), 0, TICKS - 1)
end

-- A strip with the review and block thresholds marked on it, each named
-- under its tick, and `score` lit up to where it reached, when there is
-- one.
local function marks(review, block, score, tone)
	local at_review, at_block = tick(review), tick(block)
	return ui.ticks(
		score and math.floor(score * TICKS + 0.5) or 0,
		TICKS,
		tone or "neutral",
		{ at_review, at_block },
		{ { at = at_review, text = "review" }, { at = at_block, text = "block" } }
	)
end

-- The two thresholds as numbers, small and muted.
local function thresholds(review, block)
	return ui.toned("review " .. checks.two(review) .. " · block " .. checks.two(block), "neutral", true)
end

-- The page's head: what the rules did over the repository's runs, each
-- count in its tone, and the way to add a rule.
local function intro(totals)
	local held = 0
	for _, counts in pairs(totals.per_rule) do
		held += counts[3]
	end
	local counts = {}
	if totals.runs == 0 then
		table.insert(counts, ui.toned("Nothing checked yet.", "neutral"))
	else
		table.insert(counts, ui.toned(totals.runs .. " " .. (totals.runs == 1 and "run" or "runs"), "neutral", true))
		table.insert(counts, ui.toned(totals.checked .. " checked", "neutral", true))
		for _, count in ipairs({
			{ totals.blocked, "blocked", "bad" },
			{ totals.flagged, "flagged", "warn" },
			{ held, "held", "info" },
			{ totals.failed, "not checked", "warn" },
		}) do
			if count[1] > 0 then
				table.insert(counts, ui.toned(count[1] .. " " .. count[2], count[3], true))
			end
		end
	end
	table.insert(counts, button("add a rule", "open"))
	return ui.header("Constitution", { actions = counts })
end

-- Rules that cannot be read from the store: why, what it means, and
-- the way out, removing them, once confirmed.
local function broken(why, repo, jev, confirming)
	local title
	if jev then
		title = "The rules for " .. repo .. " can't be read, so runs in " .. repo .. " fail at start"
	else
		-- Without a key no run checks rules, so none fails on them yet.
		title = "The rules for "
			.. repo
			.. " can't be read: once a TypeSafe key is added, runs in "
			.. repo
			.. " fail at start until they can"
	end
	local actions
	if confirming then
		actions = ui.row({
			ui.text("This deletes them for good."),
			button("delete", "reset", {}, "danger"),
			button("keep", "ask_reset", { yes = false }),
		})
	else
		actions = ui.row({ button("remove these rules", "ask_reset", { yes = true }, "danger") })
	end
	return ui.stack({
		ui.row({ ui.mark("failing"), ui.heading(title) }),
		ui.text(why),
		actions,
	})
end

-- Without a TypeSafe key nothing is checked: say so, and where to fix
-- it.
local function no_key()
	return ui.row({
		ui.mark("asks"),
		ui.heading("Rules aren't checked yet"),
		ui.spacer(),
		ui.link("Add TypeSafe key", "key", { plugin = "heiko-jev" }),
	})
end

-- No rules: the way to write one.
local function nothing_yet(repo)
	return ui.stack({
		ui.heading("No rules for " .. repo),
		ui.row({ button("New rule", "open", {}, "primary") }),
	})
end

-- A rule's verdict over its runs, in a few words, and its tone: blocked
-- bad, held info, flagged warn, nothing caught good.
local function verdict(rule, totals)
	local counts = totals.per_rule[rule.id] or { 0, 0, 0 }
	local blocked, flagged, held = counts[1], counts[2], counts[3]
	local parts = {}
	for _, part in ipairs({ { blocked, "blocked" }, { held, "held" }, { flagged, "flagged" } }) do
		if part[1] > 0 then
			table.insert(parts, part[1] .. " " .. part[2])
		end
	end
	local tone = "good"
	if blocked > 0 then
		tone = "bad"
	elseif held > 0 then
		tone = "info"
	elseif flagged > 0 then
		tone = "warn"
	end
	if #parts == 0 then
		return "clear", tone
	end
	return table.concat(parts, " · "), tone
end

-- The verdict as the page prints it: a met mark while the rule caught
-- nothing, else a stamp of what it caught.
local function status(rule, totals)
	local said, tone = verdict(rule, totals)
	if said == "clear" then
		return ui.row({ ui.mark("met"), ui.toned(said, tone) })
	end
	return ui.stamp(said, tone)
end

-- Where a rule reads, and what it last caught.
local function reads(rule, caught)
	local line = { ui.toned("checks", "neutral", true), ui.mono(table.concat(rule.on, ", ")) }
	if caught[rule.id] then
		table.insert(line, ui.toned("last", "neutral", true))
		table.insert(line, ui.toned(caught[rule.id], "bad", true))
	end
	return ui.row(line)
end

-- Editing a rule, and removing it.
local function rule_actions(rule)
	return ui.row({
		button("edit", "open", { rule = rule.id }),
		button("remove", "remove", { id = rule.id }, "danger"),
	})
end

-- A rule's id on a square, outlined once the rule blocked a call.
local function id_square(rule, totals)
	local counts = totals.per_rule[rule.id] or { 0, 0, 0 }
	return ui.square(rule.id, counts[1] > 0 and "outlined" or "quiet", "bad")
end

-- One rule as a ruled row: the id, the rule over where it reads, its
-- thresholds as numbers over the strip that marks them, what it caught,
-- and the ways to edit or remove it.
local function rule_row(rule, totals, caught)
	return {
		id_square(rule, totals),
		ui.stack({ ui.rich("**" .. rule.text .. "**"), reads(rule, caught) }),
		ui.stack({ thresholds(rule.review, rule.block), marks(rule.review, rule.block) }),
		status(rule, totals),
		rule_actions(rule),
	}
end

-- The columns' shares: the strip's is the row's right third.
local WIDTHS = { 0.6, 5.4, 4.5, 1.4, 1.6 }

-- The rule the page was opened on, above the list.
local function focused(rule, totals, caught)
	return ui.table({}, { rule_row(rule, totals, caught) }, { widths = WIDTHS })
end

-- The rules, one ruled row each.
local function rules_list(constitution, totals, caught)
	local rows = {}
	for _, rule in ipairs(constitution.rules) do
		table.insert(rows, rule_row(rule, totals, caught))
	end
	return ui.table({}, rows, { widths = WIDTHS })
end

-- How the checks behave beyond each rule: what happens when Jev cannot
-- answer, the one in force picked; and how many times an answer may go
-- back, a square each, which sets it to that many.
local function settings(constitution)
	local holds = constitution.max_holds
	local function set(max_holds)
		return { max_holds = max_holds }
	end
	local sent_back = { ui.text("Answers sent back per run"), ui.spacer(), button("−", "settings", set(page.next_holds(holds, -1))) }
	if holds == 0 then
		table.insert(sent_back, ui.square("0"))
	end
	for n = 1, holds do
		table.insert(sent_back, ui.square(tostring(n), "outlined", "warn", "settings", set(n)))
	end
	table.insert(sent_back, button("+", "settings", set(page.next_holds(holds, 1))))
	return ui.stack({
		ui.header("Settings"),
		ui.row({
			ui.text("When Jev can't answer"),
			ui.spacer(),
			ui.segments(
				{ { label = "let through", value = "allow" }, { label = "refuse", value = "block" } },
				constitution.on_error == "block" and "block" or "allow",
				"settings",
				{}
			),
		}),
		ui.rule(),
		ui.row(sent_back),
		ui.rule(),
	})
end

-- A flagged call or answer: what it was, the rule and how its score
-- sits against the rule's marks, and what to do with it.
local function review_card(item, rule)
	local answer = item.tool == nil
	local card = {
		ui.row({
			ui.mono(item.tool or "final answer"),
			ui.toned(item.title, "neutral"),
			ui.spacer(),
			ui.toned("p " .. checks.two(item.score), "warn", true),
		}),
		answer and ui.text(item.shown) or ui.code(nil, item.shown),
	}
	if rule then
		table.insert(card, ui.row({ ui.square(rule.id, "outlined", "warn"), ui.text(rule.text) }))
		table.insert(card, marks(rule.review, rule.block, item.score, "warn"))
	end
	table.insert(
		card,
		ui.row({
			button("looks fine", "fine", { id = item.id }, "primary"),
			button("adjust rule", "open", { rule = rule and rule.id }),
			button("open in run", "heiko.open_run", { run = item.run }),
		})
	)
	table.insert(card, ui.rule())
	return ui.stack(card)
end

-- What became of a handled call, and its tone.
local HANDLED = {
	blocked = { "blocked", "bad" },
	held = { "held", "info" },
	fine = { "looked fine", "good" },
}

-- What waits for a person, and what the rules handled on their own.
local function review(constitution, waiting, handled)
	local list = { ui.heading("Waiting for you · " .. #waiting), ui.rule() }
	if #waiting == 0 then
		table.insert(list, ui.empty("Nothing waits."))
	end
	for _, item in ipairs(waiting) do
		table.insert(list, review_card(item, rules.find(constitution, item.rule)))
	end
	table.insert(list, ui.heading("Handled by the rules"))
	if #handled == 0 then
		table.insert(list, ui.empty("Nothing yet."))
	else
		local rows = {}
		for _, done in ipairs(handled) do
			local what = HANDLED[done.what] or HANDLED.fine
			table.insert(rows, {
				ui.square(done.rule, "outlined", what[2]),
				ui.mono(done.shown),
				ui.toned(done.title, "neutral"),
				ui.stamp(what[1], what[2]),
			})
		end
		table.insert(list, ui.table({}, rows, { widths = { 0.6, 6, 3, 1.4 } }))
	end
	return ui.stack(list)
end

-- Trying the rule on the repository's latest calls, with Jev.
local function try_section(draft, trying, jev)
	local head = ui.row({
		ui.heading("Try it"),
		ui.spacer(),
		button(trying == nil and "try on recent calls" or "try again", "try"),
	})
	local body
	if trying == nil then
		if jev then
			return head
		end
		body = ui.toned("Trying a rule asks Jev: add a TypeSafe key first.", "neutral")
	elseif trying.failed then
		body = ui.row({ ui.mark("failing"), ui.text(trying.failed) })
	elseif #trying.trials == 0 then
		body = ui.empty("No calls the rule reads yet.")
	else
		local rows = {}
		for _, trial in ipairs(trying.trials) do
			local tone, would = "neutral", "would run"
			if trial.score >= draft.block then
				tone, would = "bad", "would block"
			elseif trial.score >= draft.review then
				tone, would = "warn", "would review"
			end
			table.insert(rows, {
				ui.mono(trial.tool or "answer"),
				ui.mono(checks.cut(trial.shown, checks.SHOWN_CHARS)),
				marks(draft.review, draft.block, trial.score, tone),
				ui.stamp(checks.two(trial.score) .. " " .. would, tone),
			})
		end
		body = ui.table({}, rows, { widths = { 1.5, 5, 3, 2 } })
	end
	return ui.stack({ head, body })
end

-- The rule editor, in the page's place while a rule is written: the
-- rule, where it applies, how strict it is, trying it, and saving it.
local function editor(draft, trying, jev)
	local problem = page.problem(draft)
	local shown_problem = draft.tried_to_save and problem or nil
	local title = draft.editing and ("Edit " .. draft.editing) or "New rule"
	local save = button(draft.editing and "Save" or "Add rule", "save", {}, "primary")
	local cancel = button("cancel", "close")

	local rule = { ui.heading("Rule") }
	if draft.text then
		table.insert(rule, ui.rich("**" .. draft.text .. "**"))
	end
	table.insert(
		rule,
		ui.input("rule-text", {
			placeholder = draft.text and "Write it again, then Enter" or "One sentence, then Enter",
			submit = "write",
		})
	)

	-- Where it applies: the usual places, then any other field picked.
	local offered = {}
	for _, place in ipairs(page.PLACES) do
		table.insert(offered, place)
	end
	for _, place in ipairs(draft.places) do
		local known = false
		for _, offer in ipairs(offered) do
			known = known or offer[1] == place
		end
		if not known then
			table.insert(offered, { place, "another tool's field" })
		end
	end
	local applies = { ui.heading("Applies to") }
	for _, offer in ipairs(offered) do
		local place = offer[1]
		local on = table.find(draft.places, place) ~= nil
		local dot = string.find(place, ".", 1, true)
		local label = dot and (string.sub(place, 1, dot - 1) .. " · " .. string.sub(place, dot + 1)) or "final answer"
		table.insert(
			applies,
			ui.row({
				ui.square(on and "✓" or "", on and "filled" or "quiet", "info"),
				button(label, "place", { place = place }),
				ui.toned(offer[2], "neutral"),
			})
		)
	end
	table.insert(applies, ui.input("rule-on", { placeholder = "tool.field, then Enter", submit = "add_place" }))
	if draft.bad_place then
		table.insert(applies, ui.row({ ui.mark("failing"), ui.text('"' .. draft.bad_place .. '" names no field: write tool.field.') }))
	end

	local presets = {}
	local chosen = page.preset(draft)
	for _, preset in ipairs(page.PRESETS) do
		table.insert(presets, button(preset.label, "preset", { preset = preset.name }, chosen == preset and "primary" or "secondary"))
	end
	local function stepper(name, mark, value)
		return ui.row({
			ui.toned(name, "neutral", true),
			button("−", "nudge", { mark = mark, delta = -0.05 }),
			ui.mono(checks.two(value)),
			button("+", "nudge", { mark = mark, delta = 0.05 }),
		})
	end
	local strictness = ui.stack({
		ui.heading("Strictness"),
		ui.row(presets),
		marks(draft.review, draft.block),
		ui.row({ stepper("review", "review", draft.review), ui.spacer(), stepper("block", "block", draft.block) }),
	})

	local body = {
		ui.row({ ui.title(title), ui.spacer(), cancel, save }),
		ui.rule(),
		ui.stack(rule),
		ui.stack(applies),
		strictness,
		try_section(draft, trying, jev),
	}
	if shown_problem then
		table.insert(body, ui.row({ ui.mark("failing"), ui.text(shown_problem) }))
	end
	return ui.stack(body)
end

-- The page, from what the repository's store holds (`data`): `repo`;
-- `constitution`, or `error` when it cannot be read; `jev`, whether a
-- key is saved; `focus`, the rule to show; `editor` and `trying`, the
-- rule being written and what trying it gave; `resetting`, a removal
-- waiting to be confirmed; `totals`, `waiting`, `handled` and `caught`,
-- what the rules did. The editor while a rule is written, else what
-- the rules do, what is wrong with them, and the two lists.
function page.tree(data)
	if data.editor then
		return editor(data.editor, data.trying, data.jev)
	end
	local constitution = data.constitution
	local empty = constitution ~= nil and #constitution.rules == 0
	local parts = {}
	if not empty then
		table.insert(parts, intro(data.totals))
	end
	if data.error then
		table.insert(parts, broken(data.error, data.repo, data.jev, data.resetting))
	end
	if not data.jev and data.error == nil then
		table.insert(parts, no_key())
	end
	if empty then
		table.insert(parts, nothing_yet(data.repo))
	elseif constitution then
		local rule = data.focus and rules.find(constitution, data.focus)
		if rule then
			table.insert(parts, focused(rule, data.totals, data.caught))
		end
		table.insert(
			parts,
			ui.tabs(page.TABS, {
				{ "Rules · " .. #constitution.rules, { rules_list(constitution, data.totals, data.caught), settings(constitution) } },
				{ "Review · " .. #data.waiting, { review(constitution, data.waiting, data.handled) } },
			})
		)
	end
	return ui.stack(parts)
end

return page
