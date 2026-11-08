-- Summarizing compaction (docs/reference/compaction.md, ADR 0050): when
-- a run's context nears its window, the older messages are summarized
-- in a request of their own and replaced by the summary, the newest kept
-- as they are. A context overflow summarizes at once, and an idle chat's
-- summary is asked in its own conversation, which the provider's cache
-- still holds. A failed summary past the threshold waits 2, 4, 8 and up
-- to 32 turns before it is tried again; on an overflow it fails the run.
local tau = require("tau")
local ui = tau.ui
local compaction = require("compaction")

-- One summary request of its own, with pi's instructions, on the run's
-- model at the effort the run started at: the summary, or why it is
-- none.
local function summarize(request, ctx)
	local answer = ctx.tools.infer({
		task = request,
		instructions = compaction.SYSTEM_PROMPT,
		effort = ctx.state.effort,
	})
	return compaction.summary_of(answer)
end

-- The rewrite to `summary` of what `plan` drops: the summary first, its
-- file lists after it, then the messages kept. It becomes the run's
-- latest compaction.
local function rewrite(view, plan, summary, files, ctx)
	local read_files, modified_files = compaction.file_lists(files)
	summary ..= compaction.format_files(read_files, modified_files)
	local record = {
		summary = summary,
		tokens_before = view.tokens,
		read_files = array(read_files),
		modified_files = array(modified_files),
		timestamp = ctx.now.unix * 1000,
	}
	ctx.state.compacted = record
	local keep = {}
	for at = plan.kept_from, #view.messages do
		table.insert(keep, at)
	end
	return tau.rewrite({
		keep = keep,
		insert = { { before = plan.kept_from, text = compaction.message(summary) } },
		drops_conversation = true,
		details = record,
	})
end

-- What the transcript opens with of the latest compaction: how many
-- messages are its summary, the plan of what to summarize past them, and
-- the files the summary carries.
local function planned(view, ctx)
	local record = ctx.state.compacted
	local summarized = compaction.summarized(view.messages, record)
	local plan = compaction.plan(view.messages, summarized, ctx.settings.keep_recent_tokens)
	local files = compaction.files(summarized == 1 and record or nil)
	return summarized, plan, files
end

-- Replaces the older messages with a summary: the history's, and a split
-- turn's start's, merged. Nothing when the cut keeps everything; nil and
-- why when a summary failed.
local function compact(view, ctx)
	local summarized, plan, files = planned(view, ctx)
	if plan == nil then
		return nil
	end
	local previous = summarized == 1 and ctx.state.compacted.summary or nil
	local history = compaction.slice(view.messages, plan.history)
	compaction.extract(files, history)
	local summary = previous or "No prior history."
	if #history > 0 then
		local why
		summary, why = summarize(compaction.summary_request(history, previous), ctx)
		if summary == nil then
			return nil, why
		end
	end
	if plan.turn_prefix ~= nil then
		local prefix = compaction.slice(view.messages, plan.turn_prefix)
		compaction.extract(files, prefix)
		local text, why = summarize(compaction.turn_prefix_request(prefix), ctx)
		if text == nil then
			return nil, why
		end
		summary = compaction.merge(summary, text)
	end
	return rewrite(view, plan, summary, files, ctx)
end

-- `compact`, the summary asked in the run's own conversation: the
-- messages it drops go as they are, then the prompt. One request covers
-- what a split turn would ask twice.
local function compact_in_context(view, ctx)
	local summarized, plan, files = planned(view, ctx)
	if plan == nil then
		return nil
	end
	compaction.extract(files, compaction.slice(view.messages, { summarized + 1, plan.kept_from - 1 }))
	local answer = ctx.tools.infer({
		task = compaction.in_context_request(summarized == 1),
		conversation = plan.kept_from - 1,
	})
	local summary, why = compaction.summary_of(answer)
	if summary == nil then
		return nil, why
	end
	return rewrite(view, plan, summary, files, ctx)
end

return tau.plugin({
	uses = { infer = true },
	settings = {
		schema = {
			type = "object",
			properties = {
				idle = { type = "boolean" },
				reserve_tokens = { type = "integer" },
				keep_recent_tokens = { type = "integer" },
				window = { type = "integer" },
			},
		},
		-- `window` 0 is the model's.
		default = {
			idle = true,
			reserve_tokens = compaction.RESERVE_TOKENS,
			keep_recent_tokens = compaction.KEEP_RECENT_TOKENS,
			window = 0,
		},
		view = function(settings, ctx)
			return ui.stack({
				ui.toggle("idle", "Compact idle chats"),
				ui.text(
					"A long chat left idle is summarized shortly before its prompt cache lapses, while the summary still reads it from cache: your next message then starts from the summary instead of resending everything uncached."
				),
			})
		end,
	},
	-- The effort the run starts at, which its summaries ask at.
	start = function(input, ctx)
		ctx.state.effort = input.effort
	end,
	rewrite_context = function(view, ctx)
		local state, settings = ctx.state, ctx.settings
		local window = view.window
		if settings.window ~= nil and settings.window > 0 then
			window = settings.window
		end
		if view.trigger == "overflow" then
			local rewritten, why = compact(view, ctx)
			if why ~= nil then
				error(why, 0)
			end
			return rewritten
		end
		-- Whoever offers an idle run judged it worth compacting.
		if view.trigger == "idle" then
			if not settings.idle then
				return nil
			end
			local rewritten, why = compact_in_context(view, ctx)
			if why ~= nil then
				error(why, 0)
			end
			return rewritten
		end
		local due = view.turn >= (state.retry_at or 0)
			and window ~= nil
			and compaction.should_compact(view.tokens, window, settings.reserve_tokens)
		if not due then
			return nil
		end
		local rewritten, why = compact(view, ctx)
		-- A failing summary is not paid for every turn, and a run with no
		-- turn cap is not left uncompacted for good.
		if why ~= nil then
			state.failures = (state.failures or 0) + 1
			state.retry_at = compaction.retry_at(view.turn, state.failures)
			ctx.log(why)
			return nil
		end
		state.failures = 0
		return rewritten
	end,
	view = function(state, ctx)
		return { status = ui.status("watching the window", nil, "neutral") }
	end,
})
