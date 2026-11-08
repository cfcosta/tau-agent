-- Tree compaction (docs/reference/tree-compaction.md, ADR 0050), after
-- OptChat: when a run's context nears its window, the messages before
-- the kept tail are folded into a history kept word for word, a binary
-- tree of one-line summaries grows over it, and the view, lines that
-- tile the whole history within a byte budget, takes their place. The
-- `zoom` tool opens any line into the two it was made from, down to a
-- message whole. Off by default.
local tau = require("tau")
local ui = tau.ui
local tree = require("tree")
local build = require("build")

-- What the view message says before the view.
local VIEW_PREFIX = [==[
The conversation before this point was compacted into the lines below: all of it, oldest first, as one-line summaries.

Each line is `id+n|text`: the n messages from message id on, summarized, newlines shown as spaces. A summary tags each item with its kind: user (the user's words), talk (your replies), tool (your tool calls) or echo (their results). A short message is its own line, word for word. Recent lines cover one message each; the older the messages, the more a line covers. The messages after this one are as they were.

zoom(id, n) opens line id+n into the two lines of n/2 messages it was made from; zoom(id, 1) gives message id in full. When a line only mentions something you need, such as the user's exact words, a decision, a past attempt or where a file is, zoom into it first rather than working from the summary.

<chat>
]==]

-- What closes the view message.
local VIEW_SUFFIX = "\n</chat>"

-- The most turns a failed compaction waits before it tries again.
local MAX_BACKOFF_TURNS = 32

-- The message that stands for everything folded: the view.
local function view_message(history)
	return VIEW_PREFIX .. tree.render(history) .. VIEW_SUFFIX
end

-- The run's history, as its state keeps it, or an empty one.
local function history_of(state)
	local kept = state.history
	if kept == nil then
		return tree.new()
	end
	return tree.restore(kept.entries or {}, kept.nodes or {}, kept.view or {})
end

-- `history` as its state keeps it.
local function kept(history)
	return { entries = array(history.entries), nodes = history.nodes, view = array(history.view) }
end

local SPACE = {}
for _, c in ipairs({ 0x20, 0x85, 0xA0, 0x1680, 0x2028, 0x2029, 0x202F, 0x205F, 0x3000 }) do
	SPACE[c] = true
end

-- The first message to keep so that at least `keep` tokens of the newest
-- stay, as tau-compaction cuts: back from the newest, then on to the
-- next message that is not a tool result, else the last one there is.
local function first_kept(messages, keep)
	local cuts = {}
	for at, message in ipairs(messages) do
		if message.role ~= "toolResult" then
			table.insert(cuts, at)
		end
	end
	if #cuts == 0 then
		return 1
	end
	local added = 0
	for at = #messages, 1, -1 do
		local tokens = messages[at].tokens
		if tokens ~= 0 then
			added += tokens
			if added >= keep then
				for _, cut in ipairs(cuts) do
					if cut >= at then
						return cut
					end
				end
				return cuts[#cuts]
			end
		end
	end
	return cuts[1]
end

-- Folds the messages before the kept tail into the history and puts the
-- view in their place. Nothing when the tail is all there is; nil and
-- why when a line could not be built, the history left as it was.
local function compact(view, ctx)
	local state, settings = ctx.state, ctx.settings
	local history = history_of(state)
	local messages = view.messages
	-- The transcript opens with the last view, unless another plugin's
	-- rewrite replaced it since; then whatever it opens with is folded
	-- like any other message.
	local offset = 0
	if state.history ~= nil and messages[1] ~= nil and messages[1].role == "user" and messages[1].text == view_message(history) then
		offset = 1
	end
	local rest = table.move(messages, offset + 1, #messages, 1, {})
	local kept_from = offset + first_kept(rest, settings.keep_recent_tokens)
	if kept_from - 1 <= offset then
		return nil
	end
	for at = offset + 1, kept_from - 1 do
		for _, entry in ipairs(tree.entries(messages[at])) do
			tree.push(history, entry)
		end
	end
	local function ask(request)
		return ctx.tools.infer(request)
	end
	local grown, why = build.grow(history, settings.view_bytes, ask, { jobs = settings.jobs, effort = state.effort })
	if grown == nil then
		return nil, why
	end
	state.history = kept(history)
	state.compactions = (state.compactions or 0) + 1
	state.compacted = {
		messages = kept_from - 1 - offset,
		entries = #history.entries,
		lines = #history.view,
		bytes = tree.view_bytes(history, 0),
		asked = grown.asked,
		tokens_before = view.tokens,
	}
	local keep = {}
	for at = kept_from, #messages do
		table.insert(keep, at)
	end
	return tau.rewrite({
		keep = keep,
		insert = { { before = kept_from, text = view_message(history) } },
		-- The folded messages leave the context: only zoom reaches them.
		drops_conversation = true,
		details = { view = array(history.view), entries = #history.entries, tokens_before = view.tokens },
	})
end

return tau.plugin({
	uses = { infer = true },
	settings = {
		schema = {
			type = "object",
			properties = {
				on = { type = "boolean" },
				reserve_tokens = { type = "integer" },
				keep_recent_tokens = { type = "integer" },
				window = { type = "integer" },
				view_bytes = { type = "integer" },
				jobs = { type = "integer" },
			},
		},
		-- Off until an evaluation shows it helps; `window` 0 is the
		-- model's.
		default = {
			on = false,
			reserve_tokens = 16384,
			keep_recent_tokens = 20000,
			window = 0,
			view_bytes = 64000,
			jobs = 8,
		},
		view = function(settings, ctx)
			return ui.stack({
				ui.toggle("on", "Compact into a tree"),
				ui.text(
					"Near the window, older messages fold into one-line summaries the agent can zoom back into, instead of one summary. Each long message costs a request when it folds."
				),
			})
		end,
	},
	tools = {
		zoom = {
			description = "Open the line id+n of the compacted conversation (<chat>) into the two lines of n/2 under it; n = 1 gives the message whole.",
			parameters = {
				type = "object",
				properties = {
					id = { type = "integer", minimum = 0, description = "The line's first message: the `id` of `id+n`." },
					n = { type = "integer", minimum = 0, description = "How many messages the line covers: the `n` of `id+n`." },
				},
				required = { "id", "n" },
			},
			when = "on",
			call = function(args, ctx)
				local history = history_of(ctx.state)
				if #history.entries == 0 then
					error("Nothing has been compacted yet: there is no line to open.", 0)
				end
				local text, why = tree.zoom(history, args.id, args.n)
				if text == nil then
					error(why, 0)
				end
				return text
			end,
		},
	},
	-- The effort the run starts at, which its lines are built at.
	start = function(input, ctx)
		ctx.state.effort = input.effort
	end,
	rewrite_context = function(view, ctx)
		local state, settings = ctx.state, ctx.settings
		-- An idle run is tau-compaction's to summarize, in its own
		-- conversation, while its cache lasts.
		if not settings.on or view.trigger == "idle" then
			return nil
		end
		if view.trigger == "overflow" then
			local rewritten, why = compact(view, ctx)
			if why ~= nil then
				error(why, 0)
			end
			return rewritten
		end
		local window = view.window
		if settings.window ~= nil and settings.window > 0 then
			window = settings.window
		end
		local due = view.turn >= (state.retry_at or 0)
			and window ~= nil
			and view.tokens > math.max(window - settings.reserve_tokens, 0)
		if not due then
			return nil
		end
		local rewritten, why = compact(view, ctx)
		-- When a line cannot be built the tree writes nothing, and the
		-- summary after it steps in.
		if why ~= nil then
			state.failures = (state.failures or 0) + 1
			state.retry_at = view.turn + math.min(2 ^ state.failures, MAX_BACKOFF_TURNS)
			ctx.log(why)
			return nil
		end
		state.failures = 0
		return rewritten
	end,
	view = function(state, ctx)
		local line
		if not ctx.settings.on then
			line = "off"
		elseif (state.compactions or 0) == 0 then
			line = "watching the window"
		else
			line = state.compacted.entries .. " messages in " .. state.compacted.lines .. " lines"
		end
		return { status = ui.status(line, nil, "neutral") }
	end,
})
