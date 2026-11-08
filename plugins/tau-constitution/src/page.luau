-- A repository's rules page: what each rule reads, how strict it is
-- and what it did, the calls and answers that wait for a person, and
-- the editor that writes and tries a rule. The rule being written, what
-- trying it gave and a removal waiting to be confirmed are kept in the
-- repository's store, as the rules are.
local tau = require("tau")
local ui = tau.ui
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

-- Asks `jev` what a rule being written makes of past `calls` and
-- `answers`, as a check would ask about each, one request each: the
-- trials, `{ tool, shown, score }`, and what Jev cost; else nil and
-- why. Nothing runs again; Jev sees only what a check would show it.
function page.try(jev, draft, calls, answers)
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
		local scores, spent = checks.ask(jev, ask.request, 1)
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

-- A field's name, and what it is for.
local function label_row(name, hint)
	return ui.row({ ui.heading(name), ui.text(hint) })
end

-- What the rules did over the repository's runs, each count in its
-- tone, and the way to add a rule.
local function intro(totals)
	local held = 0
	for _, counts in pairs(totals.per_rule) do
		held += counts[3]
	end
	local counts = {}
	if totals.runs == 0 then
		table.insert(counts, ui.text("Nothing checked yet."))
	else
		table.insert(counts, ui.text("Across " .. totals.runs .. " " .. (totals.runs == 1 and "run" or "runs") .. ":"))
		table.insert(counts, ui.pill(totals.checked .. " checked", "neutral"))
		for _, count in ipairs({
			{ totals.blocked, "blocked", "bad" },
			{ totals.flagged, "flagged", "warn" },
			{ held, "held", "info" },
			{ totals.failed, "not checked", "warn" },
		}) do
			if count[1] > 0 then
				table.insert(counts, ui.pill(count[1] .. " " .. count[2], count[3]))
			end
		end
		if totals.cost > 0 then
			table.insert(counts, ui.text("Jev cost"))
			table.insert(counts, ui.mono(totals.cost < 0.01 and string.format("$%.5f", totals.cost) or checks.usd(totals.cost)))
		end
	end
	return ui.stack({
		ui.row({
			ui.rich("**tau-constitution** checks every tool call and final answer against these rules before they run."),
			button("Add a rule", "open"),
		}),
		ui.row(counts),
	})
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
			ui.text("This deletes them for good, so you can write them again."),
			button("Delete", "reset", {}, "danger"),
			button("Keep", "ask_reset", { yes = false }),
		})
	else
		actions = ui.row({ button("Remove these rules", "ask_reset", { yes = true }, "danger") })
	end
	return ui.stack({
		ui.row({ ui.icon("blocked", "bad"), ui.heading(title) }),
		ui.text(why),
		actions,
	})
end

-- Without a TypeSafe key nothing is checked: say so, and where to fix
-- it.
local function no_key()
	return ui.row({
		ui.icon("key", "info"),
		ui.stack({
			ui.heading("Rules aren't checked yet"),
			ui.text("tau-constitution asks Jev, TypeSafe's scoring model. Add a TypeSafe key and every run checks these rules."),
		}),
		ui.link("Add TypeSafe key", "key", { plugin = "tau-jev" }),
	})
end

-- No rules: what a rule is, and how to write one.
local function nothing_yet(repo)
	return ui.stack({
		ui.icon("blocked", "neutral"),
		ui.heading("No rules for " .. repo),
		ui.text(
			"A rule is one sentence, checked where you say: a shell command, an edit, a written file, or the final answer. Past its block mark the call is refused and the model gets the rule back."
		),
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

-- Where a rule reads and how strict it is, and what it last caught.
local function caption(rule, caught)
	local text = "Checks "
		.. table.concat(rule.on, ", ")
		.. " · flag "
		.. checks.two(rule.review)
		.. " · block "
		.. checks.two(rule.block)
	if caught[rule.id] then
		text ..= " · last: " .. caught[rule.id]
	end
	return text
end

-- Editing a rule, and removing it.
local function rule_actions(rule)
	return ui.row({
		button("Edit", "open", { rule = rule.id }),
		button("Remove", "remove", { id = rule.id }, "danger"),
	})
end

-- The rule the page was opened on, above the list.
local function focused(rule, totals, caught)
	local said, tone = verdict(rule, totals)
	return ui.stack({
		ui.row({ ui.mono(rule.id), ui.heading(rule.text), ui.badge(said, tone) }),
		ui.text(caption(rule, caught)),
		ui.progress(rule.block, "flags from " .. checks.two(rule.review) .. ", blocks from " .. checks.two(rule.block)),
		rule_actions(rule),
	})
end

-- The rules, one row each: the id, the rule with where it reads and how
-- strict it is, what it caught, and the ways to edit or remove it.
local function rules_list(constitution, totals, caught)
	local rows = {}
	for _, rule in ipairs(constitution.rules) do
		local said, tone = verdict(rule, totals)
		table.insert(rows, {
			ui.mono(rule.id),
			ui.stack({ ui.text(rule.text), ui.text(caption(rule, caught)) }),
			ui.badge(said, tone),
			rule_actions(rule),
		})
	end
	return ui.table({ "Rule", "", "Caught", "" }, rows)
end

-- How the checks behave beyond each rule: what happens when Jev cannot
-- answer, and how many times an answer may go back, with a note on what
-- Jev reads.
local function settings(constitution)
	local blocks, holds = constitution.on_error == "block", constitution.max_holds
	local function set(blocks_unchecked, max_holds)
		return { blocks_unchecked = blocks_unchecked, max_holds = max_holds }
	end
	local detail = blocks and "Calls it could not check are refused, and answers sent back while holds are left."
		or "Calls it could not check run, and answers stand; each is noted."
	local sent_back = holds == 0 and "A final answer that breaks a rule stands, flagged."
		or ("Up to " .. holds .. "; past that, an answer that breaks a rule stands, flagged.")
	return ui.stack({
		ui.heading("Settings"),
		ui.row({
			ui.stack({ ui.text("Refuse what Jev can't answer"), ui.text(detail) }),
			button(blocks and "On" or "Off", "settings", set(not blocks, holds), blocks and "primary" or "secondary"),
		}),
		ui.row({
			ui.stack({ ui.text("Answers sent back per run"), ui.text(sent_back) }),
			button("−", "settings", set(blocks, page.next_holds(holds, -1))),
			ui.mono(tostring(holds)),
			button("+", "settings", set(blocks, page.next_holds(holds, 1))),
		}),
		ui.text("Jev sees only the fields a rule names, as the model wrote them. Never tool output or file contents."),
	})
end

-- A flagged call or answer: what it was, the rule and how its score
-- sits against the rule's marks, and what to do with it.
local function review_card(item, rule)
	local answer = item.tool == nil
	local card = {
		ui.row({
			ui.icon(answer and "chat" or "runs", "neutral"),
			ui.mono(item.tool or "final answer"),
			ui.text("in " .. item.title),
			ui.badge("p " .. checks.two(item.score), "warn"),
		}),
		answer and ui.text(item.shown) or ui.code(nil, item.shown),
	}
	if rule then
		table.insert(card, ui.row({ ui.mono(rule.id), ui.text(rule.text) }))
		table.insert(
			card,
			ui.progress(
				item.score,
				"p "
					.. checks.two(item.score)
					.. " · flags from "
					.. checks.two(rule.review)
					.. ", blocks from "
					.. checks.two(rule.block)
			)
		)
	end
	table.insert(
		card,
		ui.text(
			answer and "The answer stood, but its score is between this rule's flag and block marks."
				or "It ran: its score is between this rule's flag and block marks."
		)
	)
	table.insert(
		card,
		ui.row({
			button("Adjust rule", "open", { rule = rule and rule.id }),
			button("Open in run", "tau.open_run", { run = item.run }),
			button("Looks fine", "fine", { id = item.id }, "primary"),
		})
	)
	return ui.stack(card)
end

local HANDLED = {
	blocked = { "blocked", "bad", "blocked" },
	held = { "chat", "info", "held" },
	fine = { "check", "good", "looked fine" },
}

-- What waits for a person, and what the rules handled on their own.
local function review(constitution, waiting, handled)
	local list = { ui.heading("Waiting for you · " .. #waiting) }
	if #waiting == 0 then
		table.insert(list, ui.empty("Nothing waits for you. Flagged calls land here."))
	end
	for _, item in ipairs(waiting) do
		table.insert(list, review_card(item, rules.find(constitution, item.rule)))
	end
	table.insert(list, ui.heading("Handled by the rules"))
	if #handled == 0 then
		table.insert(list, ui.text("Nothing yet."))
	else
		local rows = {}
		for _, done in ipairs(handled) do
			local glyph = HANDLED[done.what] or HANDLED.fine
			table.insert(rows, {
				ui.icon(glyph[1], glyph[2]),
				ui.stack({ ui.mono(done.shown), ui.text(done.title) }),
				ui.mono(done.rule .. " · " .. glyph[3]),
			})
		end
		table.insert(list, ui.table({}, rows))
	end
	table.insert(
		list,
		ui.text(
			"Blocked calls need nothing from you: the model got the rule and changed the call. They show that a rule does its job."
		)
	)
	return ui.stack(list)
end

-- Trying the rule on the repository's latest calls, with Jev.
local function try_section(draft, trying, jev)
	local head = ui.row({
		label_row("Try it", "before it runs for real"),
		button(trying == nil and "Try on recent calls" or "Try again", "try"),
	})
	local body
	if trying == nil then
		body = ui.text(
			jev and "Jev scores the latest calls and answers this rule would read, as a check would. Nothing runs again."
				or "Trying a rule asks Jev: add a TypeSafe key first."
		)
	elseif trying.failed then
		body = ui.row({ ui.icon("warning", "bad"), ui.text(trying.failed) })
	elseif #trying.trials == 0 then
		body = ui.text("No calls in this repository's runs that the rule reads yet.")
	else
		local rows = {}
		for _, trial in ipairs(trying.trials) do
			local tone, would = "neutral", "would run"
			if trial.score >= draft.block then
				tone, would = "bad", "would block"
			elseif trial.score >= draft.review then
				tone, would = "warn", "would flag"
			end
			table.insert(rows, {
				ui.mono(trial.tool or "answer"),
				ui.mono(checks.cut(trial.shown, checks.SHOWN_CHARS)),
				ui.progress(trial.score),
				ui.badge(checks.two(trial.score) .. " " .. would, tone),
			})
		end
		local n = #trying.trials
		body = ui.stack({
			ui.table({}, rows),
			ui.text(
				n
					.. " "
					.. (n == 1 and "call" or "calls")
					.. " · "
					.. checks.usd(trying.cost)
					.. ". Only what a check shows went to Jev; nothing ran again."
			),
		})
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
	local cancel = button("Cancel", "close")

	local rule = { label_row("Rule", "one sentence, the way you would tell a person") }
	if draft.text then
		table.insert(rule, ui.row({ ui.text("Rule:"), ui.mono(draft.text) }))
	end
	table.insert(
		rule,
		ui.input("rule-text", {
			placeholder = draft.text and "Type to write it again; Enter keeps it"
				or 'A rule in plain words: "No unwrap or expect outside tests." Enter keeps it',
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
	local applies = { label_row("Applies to", "what Jev reads for this rule") }
	for _, offer in ipairs(offered) do
		local place = offer[1]
		local on = table.find(draft.places, place) ~= nil
		local dot = string.find(place, ".", 1, true)
		local label = dot and (string.sub(place, 1, dot - 1) .. " · " .. string.sub(place, dot + 1)) or "Final answer"
		table.insert(
			applies,
			ui.row({ button((on and "✓ " or "") .. label, "place", { place = place }, on and "primary" or "secondary"), ui.text(offer[2]) })
		)
	end
	table.insert(applies, ui.input("rule-on", { placeholder = "tool.field, then Enter to add it", submit = "add_place" }))
	if draft.bad_place then
		table.insert(applies, ui.row({ ui.icon("warning", "bad"), ui.text('"' .. draft.bad_place .. '" names no field: write tool.field.') }))
	end

	local presets = {}
	local chosen = page.preset(draft)
	for _, preset in ipairs(page.PRESETS) do
		table.insert(presets, button(preset.label, "preset", { preset = preset.name }, chosen == preset and "primary" or "secondary"))
	end
	local function stepper(name, mark, value)
		return ui.row({
			ui.text(name),
			button("−", "nudge", { mark = mark, delta = -0.05 }),
			ui.mono(checks.two(value)),
			button("+", "nudge", { mark = mark, delta = 0.05 }),
		})
	end
	local strictness = ui.stack({
		label_row("Strictness", "how sure Jev must be that the rule is broken"),
		ui.row(presets),
		ui.progress(draft.block, "flags from " .. checks.two(draft.review) .. ", blocks from " .. checks.two(draft.block)),
		ui.row({ stepper("Flag at", "review", draft.review), stepper("Block at", "block", draft.block) }),
		ui.rows({
			{ "Below " .. checks.two(draft.review), "The call runs." },
			{ checks.two(draft.review) .. " to " .. checks.two(draft.block), "It runs, and waits in Review for you." },
			{ checks.two(draft.block) .. " and up", "Refused. The model gets the rule and tries again." },
		}),
	})

	local body = {
		ui.row({ ui.heading(title), cancel, save }),
		ui.stack(rule),
		ui.stack(applies),
	}
	if shown_problem then
		table.insert(body, ui.row({ ui.icon("warning", "bad"), ui.text(shown_problem) }))
	end
	table.insert(body, strictness)
	table.insert(body, try_section(draft, trying, jev))
	local footer
	if shown_problem then
		footer = shown_problem
	elseif draft.editing then
		footer = "Saves over " .. draft.editing .. "; runs check with it from their next tool call"
	else
		footer = "Adds it; runs check with it from their next tool call"
	end
	table.insert(body, ui.row({ ui.text(footer), cancel, save }))
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
