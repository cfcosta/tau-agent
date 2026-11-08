-- Keeps a run's context lean with Jev, in two stages
-- (docs/reference/fast-compaction.md, ADR 0050):
--
-- - output pruning, at `after_tool_result`: a large `exec_command` or
--   `write_stdin` result is trimmed to the chunks of lines Jev says the
--   task still needs, before the model first sees it, with the whole
--   output archived to a file the model can read; after
--   `tamaratran/jev-pruner`;
-- - history pruning, between turns: stale tool calls and results are
--   dropped or cut, as a context rewrite; a port of
--   `joelhooks/pi-fast-jev-compaction` (see THIRD_PARTY_NOTICES.md).
--
-- Without Jev it does nothing.
local tau = require("tau")
local ui = tau.ui
local estimate = require("estimate")
local state = require("state")
local decide = require("decide")
local ledger = require("ledger")
local output = require("output")

local NAME = "tau-fast-compaction"

-- What the status line says without a TypeSafe key.
local NO_KEY = "off · no TypeSafe key"

-- Where a pass runs between turns, as a share of the window, by default.
local COMPACT_AT = 60

local DEFAULT = {
	goal = "",
	keep_threshold = 0.5,
	preserve_recent = 6,
	max_state_tokens = 25000,
	max_request_tokens = 30000,
	head_chars = 300,
	compact_at_percent = COMPACT_AT,
	min_reduction_ratio = 0.25,
	cooldown_percent = 10,
	start_reserve_tokens = 16384,
	window = 0,
	output_enabled = true,
	output_min_tokens = 10000,
	output_chunk_lines = 20,
	output_keep_threshold = 0.5,
	output_max_state_tokens = 25000,
	output_max_request_tokens = 30000,
	output_max_requests = 12,
	output_min_reduction_ratio = 0.1,
}

local SCHEMA = { type = "object", properties = {} }
for key, value in pairs(DEFAULT) do
	local kind = type(value) == "boolean" and "boolean" or type(value) == "string" and "string" or "number"
	SCHEMA.properties[key] = { type = kind }
end

-- The settings, the defaults under what the person set.
local function settings_of(ctx)
	local out = table.clone(DEFAULT)
	for key, value in pairs(ctx.settings or {}) do
		out[key] = value
	end
	return out
end

-- The window a pass measures by: the settings', else the model's.
local function window_of(view, settings)
	if settings.window ~= nil and settings.window > 0 then
		return settings.window
	end
	return view.window
end

-- Whether a pass is due between turns: past the share of the window, and
-- past the cooldown since the last pass that asked Jev.
local function due(ctx, tokens, window, settings)
	if window == nil or window <= 0 then
		return false
	end
	local last = ctx.state.last_pass
	local cooled = last == nil or math.max(tokens - last, 0) >= window * settings.cooldown_percent / 100
	return tokens * 100 / window >= settings.compact_at_percent and cooled
end

-- Whether the context is within the start reserve of the window.
local function overfull(tokens, window, settings)
	return window ~= nil and window > 0 and tokens > math.max(window - settings.start_reserve_tokens, 0)
end

local function count(n, one, many)
	return n .. " " .. (n == 1 and one or many)
end

-- `184000` as `184k`.
local function tokens_text(n)
	if n < 1000 then
		return tostring(n)
	elseif n < 1000000 then
		return (n // 1000) .. "k"
	end
	return string.format("%.1fM", n / 1000000)
end

-- A Jev cost, often a fraction of a cent: to four places under a cent,
-- else to the cent, or to a tenth of one under a dollar.
local function dollars(amount)
	if amount < 0.01 then
		return string.format("$%.4f", amount)
	end
	local fine = string.format("%.3f", amount)
	if tonumber(fine) < 1 then
		return "$" .. fine
	end
	return string.format("$%.2f", amount)
end

-- What a pass judged and what Jev cost: `12 calls judged in 1 Jev
-- request · …`.
local function detail(stats)
	local text = count(stats.calls - stats.pinned, "call", "calls")
		.. " judged in "
		.. count(stats.requests, "Jev request", "Jev requests")
		.. " · "
		.. count(stats.results_dropped, "result", "results")
		.. " cut, "
		.. count(stats.calls_dropped, "call", "calls")
		.. " dropped · −"
		.. string.format("%.0f", stats.reduction_ratio * 100)
		.. "% · Jev read "
		.. tokens_text(stats.state_tokens)
		.. " tokens of history, "
		.. stats.state_stage
	if stats.cost > 0 then
		text ..= " · " .. dollars(stats.cost)
	end
	return text
end

-- A pass between turns: asks Jev about every unpinned call, merges the
-- decisions into the ledger, and rewrites the transcript when that saves
-- enough. Nothing when there is nothing to ask or too little to save;
-- nil and why on a failure.
local function pass(view, ctx, settings)
	local preserve = math.max(settings.preserve_recent, 1)
	local entries = state.entries(view.messages)
	local calls = state.calls(entries, preserve)
	local unpinned = false
	for _, call in ipairs(calls) do
		unpinned = unpinned or not call.pinned
	end
	if not unpinned then
		return nil
	end
	-- Unlike pi, the cooldown starts only with a pass that asks Jev.
	ctx.state.last_pass = view.tokens
	local decided, why, spent = decide.decide(ctx.jev, entries, calls, settings)
	if decided == nil then
		return nil, why
	end
	local staged = ledger.of(ctx.state.decisions)
	ledger.merge(staged, decided.decisions)
	local archives = ledger.assign_archives(staged, view.messages, settings.head_chars, ctx.archive.path)
	local rewrite = ledger.apply(staged, view.messages, settings.head_chars)
	local measured = ctx.measure(rewrite)
	local reduction = 0
	if measured.before > 0 then
		reduction = math.max(measured.before - measured.after, 0) / measured.before
	end
	if reduction < settings.min_reduction_ratio then
		return nil
	end
	-- Archives are written only for a rewrite that happens; one that
	-- cannot be written fails the pass rather than cut a result the
	-- model could not read back.
	for _, archive in ipairs(archives) do
		local ok, failed = pcall(ctx.archive.write, archive.path, archive.text)
		if not ok then
			return nil, tostring(failed)
		end
	end
	local function counted(action)
		local n = 0
		for at, decision in ipairs(decided.decisions) do
			if not calls[at].pinned and decision.action == action then
				n += 1
			end
		end
		return n
	end
	local pinned = 0
	for _, call in ipairs(calls) do
		pinned += call.pinned and 1 or 0
	end
	local stats = {
		calls = #calls,
		pinned = pinned,
		kept = counted("keep"),
		results_dropped = counted("drop_result"),
		calls_dropped = counted("drop_call"),
		requests = decided.requests,
		state_tokens = decided.state_tokens,
		state_stage = decided.segments == 1 and "whole" or (decided.segments .. " segments"),
		chars_before = measured.before,
		chars_after = measured.after,
		reduction_ratio = reduction,
		cost = spent,
	}
	local decisions = ledger.list(staged)
	ctx.state.decisions = array(decisions)
	-- What the ledger page and the cards show of this pass: each call,
	-- what it was decided, and what it takes.
	-- Calls an earlier pass dropped have left the transcript; they still
	-- show, first, as they came first.
	local judged = {}
	local present = {}
	for _, call in ipairs(calls) do
		present[call.call_id] = true
	end
	for _, decision in ipairs(decisions) do
		if not present[decision.call_id] then
			table.insert(judged, {
				call_id = decision.call_id,
				tool = decision.tool,
				input = "",
				tokens = 0,
				matters = decision.keep_call,
				verbatim = decision.keep_result,
				decision = decision.action,
			})
		end
	end
	for _, call in ipairs(calls) do
		local decision = staged[call.call_id]
		table.insert(judged, {
			call_id = call.call_id,
			tool = call.tool,
			input = estimate.truncate(call.input, 120),
			tokens = estimate.div_ceil(utf8.len(call.input) + call.result_chars, 4),
			matters = decision and decision.keep_call or json.null,
			verbatim = decision and decision.keep_result or json.null,
			decision = decision and decision.action or "pinned",
		})
	end
	local passes = ctx.state.passes or {}
	table.insert(passes, {
		before = estimate.div_ceil(stats.chars_before, 4),
		after = estimate.div_ceil(stats.chars_after, 4),
		results_dropped = stats.results_dropped,
		calls_dropped = stats.calls_dropped,
		reduction_ratio = reduction,
		detail = detail(stats),
		window = window_of(view, settings) or json.null,
	})
	ctx.state.passes = array(passes)
	ctx.state.ledger = array(judged)
	rewrite.details = { decisions = array(decisions), stats = stats }
	return tau.rewrite(rewrite)
end

-- Prunes one output that passed the gate: plans the requests, asks Jev,
-- renders what stays, and writes the archive. The pruned text, when it
-- saves at least the ratio of the output; nil and why on a failure.
-- Reports how it went whenever Jev was asked.
local function prune(result, full, ctx, settings)
	local entries = state.entries(ctx.transcript())
	local task = state.goal_or_prompts(settings.goal, entries)
	local records = output.records(entries)
	local lines = output.lines(full)
	local pruning = {
		chunk_lines = settings.output_chunk_lines,
		max_state_tokens = settings.output_max_state_tokens,
		max_request_tokens = settings.output_max_request_tokens,
		max_output_requests = settings.output_max_requests,
	}
	local planned, why = output.plan(lines, records, task, output.command_of(result), pruning)
	if planned == nil then
		return nil, why
	end
	if #planned.requests == 0 then
		return nil
	end
	local asked, failed, spent = output.ask(ctx.jev, planned, settings.output_keep_threshold)
	if asked == nil then
		return nil, failed
	end
	local archive = ctx.archive.path("output")
	local text = output.render(lines, planned.chunks, asked.keep, archive)
	local before, after = estimate.tokens(full), estimate.tokens(text)
	local kept, dropped_lines = 0, 0
	for index, chunk in ipairs(planned.chunks) do
		if asked.keep[index] then
			kept += 1
		else
			dropped_lines += chunk[2] - chunk[1] + 1
		end
	end
	local pruned = dropped_lines > 0 and after <= before * (1 - settings.output_min_reduction_ratio)
	if pruned then
		local ok, failure = pcall(ctx.archive.write, archive, full)
		if not ok then
			return nil, tostring(failure)
		end
	end
	local outputs = ctx.state.outputs or {}
	table.insert(outputs, {
		call_id = result.id,
		lines = #lines,
		chunks = #planned.chunks,
		kept = kept,
		dropped_lines = dropped_lines,
		segments = planned.segments,
		requests = asked.requests,
		tokens_before = before,
		tokens_after = pruned and after or before,
		pruned = pruned,
		archive = pruned and archive or json.null,
		cost = spent,
	})
	ctx.state.outputs = array(outputs)
	return pruned and text or nil
end

-- What a call's card says of what the last pass made of it.
local CARD = { drop_result = "result dropped", drop_call = "call dropped" }

-- What each decision reads as in the ledger.
local LABEL = { pinned = "pinned", keep = "keep", drop_result = "drop result", drop_call = "drop call" }

local TONE = { pinned = "neutral", keep = "neutral", drop_result = "info", drop_call = "warn" }

-- The ledger page's link.
local function ledger_link()
	return ui.link("Ledger", "luau-plugins", { plugin = "tau-luau-plugins", params = { entry = NAME } })
end

-- What the run's context was before and after the last pass, and what it
-- decided for each tool call.
local function ledger_page(state_now)
	local passes = state_now.passes or {}
	local last = passes[#passes]
	if last == nil then
		return { ui.empty(NAME .. " has not pruned the context of this run yet.") }
	end
	local window = last.window ~= json.null and last.window or math.max(last.before, 1)
	local kept, pinned, results, calls = 0, 0, 0, 0
	for _, entry in ipairs(state_now.ledger or {}) do
		if entry.decision == "keep" then
			kept += 1
		elseif entry.decision == "pinned" then
			pinned += 1
		elseif entry.decision == "drop_result" then
			results += 1
		else
			calls += 1
		end
	end
	local saved = 0
	for _, cut in ipairs(state_now.outputs or {}) do
		if cut.pruned then
			saved += cut.tokens_before - cut.tokens_after
		end
	end
	local pruned_outputs = 0
	for _, cut in ipairs(state_now.outputs or {}) do
		pruned_outputs += cut.pruned and 1 or 0
	end
	local rows = {}
	for _, entry in ipairs(state_now.ledger or {}) do
		local odds = function(p)
			return p == json.null and "·" or string.format("%.2f", p)
		end
		table.insert(rows, {
			ui.mono(entry.tool),
			ui.mono(entry.input),
			ui.mono(tokens_text(entry.tokens)),
			ui.mono(odds(entry.matters)),
			ui.mono(odds(entry.verbatim)),
			ui.badge(LABEL[entry.decision], TONE[entry.decision]),
		})
	end
	return {
		ui.heading("Pruned " .. tokens_text(math.max(last.before - last.after, 0)) .. " tokens of this run's tool history"),
		ui.text(
			last.detail
				.. ". Decisions only escalate: keep, then drop the result, then drop the call. Forks inherit this ledger."
		),
		ui.progress(last.before / window, "Before · " .. tokens_text(last.before)),
		ui.progress(last.after / window, "After · " .. tokens_text(last.after)),
		ui.text("Window " .. tokens_text(window) .. ". Pruning starts at " .. COMPACT_AT .. "%."),
		ui.rule(),
		ui.rows({
			{ "plugin", NAME },
			{ "pinned", tostring(pinned) },
			{ "kept", tostring(kept) },
			{ "result dropped", tostring(results) },
			{ "call dropped", tostring(calls) },
			{ "next request", "1 full resend" },
			{
				"outputs pruned",
				pruned_outputs == 0 and "none" or (pruned_outputs .. " · −" .. tokens_text(saved) .. " tokens"),
			},
		}),
		ui.heading("Ledger"),
		#rows == 0 and ui.empty("The pass reported no per-call decisions.")
			or ui.table({ "Tool", "Input", "Size", "Still matters", "Verbatim", "Decision" }, rows),
	}
end

return tau.plugin({
	uses = { jev = true, transcript = true, archive = true },
	settings = {
		schema = SCHEMA,
		default = DEFAULT,
		view = function(settings, ctx)
			return ui.stack({
				ui.text(
					"Prunes large command outputs as they arrive, and stale tool history between turns, asking Jev what the task still needs. What it cuts is archived for the model to read back. It needs a TypeSafe key."
				),
			})
		end,
	},
	-- Whether it can prune: there is a TypeSafe key.
	start = function(input, ctx)
		ctx.state.on = ctx.jev ~= nil
	end,
	rewrite_context = function(view, ctx)
		if ctx.jev == nil then
			return nil
		end
		local settings = settings_of(ctx)
		local window = window_of(view, settings)
		local trigger = view.trigger
		local wanted
		if trigger == "overflow" then
			wanted = true
		elseif trigger == "start" then
			-- A fork reads its parent's transcript from the parent's
			-- prompt cache: only a rewrite that must fit the window runs.
			wanted = overfull(view.tokens, window, settings)
		elseif trigger == "turn_end" then
			wanted = due(ctx, view.tokens, window, settings)
		else
			-- An idle run is summarized in its own conversation while its
			-- cache lasts: pruning first would change what that reads.
			wanted = false
		end
		if not wanted then
			return nil
		end
		local rewritten, why = pass(view, ctx, settings)
		if why ~= nil then
			if trigger == "overflow" then
				error(why, 0)
			end
			ctx.log(why)
			return nil
		end
		return rewritten
	end,
	after_tool_result = function(result, ctx)
		local settings = settings_of(ctx)
		if ctx.jev == nil or not settings.output_enabled or result.nested then
			return nil
		end
		local full = output.gate(result, settings.output_min_tokens)
		if full == nil then
			return nil
		end
		local text, why = prune(result, full, ctx, settings)
		if why ~= nil then
			ctx.log(why)
			return nil
		end
		if text == nil then
			return nil
		end
		return tau.result(text, { flag = false })
	end,
	view = function(state_now, ctx)
		local passes = state_now.passes or {}
		local last = passes[#passes]
		local line
		if last ~= nil then
			line = (last.results_dropped + last.calls_dropped)
				.. " pruned · −"
				.. string.format("%.0f", last.reduction_ratio * 100)
				.. "%"
		elseif state_now.on == false then
			line = NO_KEY
		elseif state_now.on then
			line = "pruning large outputs · watching the window"
		end
		local view = { calls = {}, notes = {} }
		if line ~= nil then
			view.status = ui.status(line, nil, "neutral")
		end
		-- Each pass where it rewrote the transcript.
		for at, one in ipairs(passes) do
			view.notes["pass-" .. at] = ui.stack({
				ui.row({
					ui.icon("plug", "info"),
					ui.mono(NAME),
					ui.text(
						"pruned "
							.. tokens_text(math.max(one.before - one.after, 0))
							.. " tokens: "
							.. tokens_text(one.before)
							.. " to "
							.. tokens_text(one.after)
					),
					ledger_link(),
				}),
				ui.mono(one.detail),
				ui.text("The next request resends the pruned transcript once, then turns are deltas again."),
			})
		end
		-- What the last pass left of each call, on its card.
		local kept = 0
		for _, entry in ipairs(state_now.ledger or {}) do
			if CARD[entry.decision] ~= nil then
				view.calls[entry.call_id] = {
					badge = ui.badge(CARD[entry.decision], "neutral"),
					dropped = entry.decision == "drop_result" and "result" or "call",
				}
			else
				kept += 1
			end
		end
		-- An output cut as it arrived: its card says what the model saw,
		-- and where the whole output is.
		for _, cut in ipairs(state_now.outputs or {}) do
			if cut.pruned then
				view.calls[cut.call_id] = view.calls[cut.call_id] or {}
				view.calls[cut.call_id].cut = {
					kept = cut.lines - cut.dropped_lines,
					lines = cut.lines,
					archive = cut.archive,
					tokens_before = cut.tokens_before,
					tokens_after = cut.tokens_after,
					cost = cut.cost,
				}
			end
		end
		if last ~= nil then
			view.inspector = ui.row({ ui.text(kept .. " kept by " .. NAME .. "."), ledger_link() })
		end
		view.page = ledger_page(state_now)
		return view
	end,
})
