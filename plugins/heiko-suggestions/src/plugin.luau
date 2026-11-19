-- Suggestions for the next run (ADR 0063): when a run in a repository
-- ends, what it was about is kept in the repository's store, and a tick
-- asks the plan's smallest model what the person may ask next. The
-- newest is what the empty composer of a new run there says; Tab writes
-- it. Nothing waits on the model: the run ends as it would have.
local heiko = require("heiko")
local suggest = require("suggest")
local cmd = heiko.cmd
local ui = heiko.ui

-- The model family asked, at low effort, as titles are.
local FAMILY = "luna"

-- How many to keep, from the settings.
local function count_of(settings)
	local count = tonumber(settings.count) or suggest.COUNT
	return math.clamp(math.floor(count), 1, suggest.MAX_COUNT)
end

-- How long ago `at` was, at `now`, in a word or two.
local function ago(at, now)
	local seconds = math.max(0, now - at)
	if seconds < 3600 then
		return math.max(1, seconds // 60) .. " min ago"
	elseif seconds < 86400 then
		return seconds // 3600 .. " h ago"
	end
	return seconds // 86400 .. " d ago"
end

-- Asks for the suggestions of the request `request` kept in `store`,
-- replacing its pending ones: whether any came.
local function answer(store, request, env)
	store.delete("request")
	local count = count_of(env.settings)
	local list = store.get("suggestions") or {}
	local answered = heiko.infer({
		task = suggest.request(request.digest, suggest.turned_down(list), count),
		context = json.null,
		schema = suggest.schema(count),
		effort = "low",
	})
	if not answered.ok then
		heiko.log("asking for suggestions: " .. tostring(answered.error))
		return false
	end
	-- Read again: the person may have taken or dismissed one meanwhile.
	list = store.get("suggestions") or {}
	local merged, added = suggest.merge(
		list,
		(answered.value or {}).suggestions,
		request.run,
		suggest.model_of(env.models, FAMILY),
		heiko.now(env),
		count
	)
	store.put("suggestions", merged)
	store.put("last", request)
	return added > 0
end

return heiko.plugin({
	uses = { infer = true, model = FAMILY, store = true, chats = true, transcript = true },
	settings = {
		schema = {
			type = "object",
			properties = {
				on = {
					type = "boolean",
					title = "Suggest the next run",
					description = "After a run ends, one short request to the plan's smallest model; the empty composer of a new run says the newest. Off, it keeps its own words.",
				},
				count = {
					type = "integer",
					minimum = 1,
					maximum = suggest.MAX_COUNT,
					title = "How many to keep",
				},
			},
		},
		default = { on = true, count = suggest.COUNT },
	},

	-- What the run was about, for the tick to ask from: the model is not
	-- asked here, so the run ends as soon as it would have.
	run_end = function(ended, _, env)
		if not env.settings.on or env.run.kind == "sub_agent" or ended.stop == "Cancelled" then
			return nil
		end
		local digest = suggest.digest(heiko.await(cmd.transcript()))
		if digest == nil then
			return nil
		end
		heiko.store.put("request", { run = env.run.id, at = heiko.now(env), digest = digest })
		return nil
	end,

	-- A run that starts with a suggestion's prompt took it.
	start = function(plan)
		if type(plan.input) ~= "string" then
			return nil
		end
		local list, taken = suggest.mark(heiko.store.get("suggestions") or {}, plan.input, "taken")
		if taken then
			heiko.store.put("suggestions", list)
		end
		return nil
	end,

	-- Every few seconds, each repository's request is asked. One tick at
	-- a time: a slow answer holds the next back, not the person.
	jobs = {
		suggest = {
			every = 5,
			label = "Suggesting the next run",
			run = function(_, model, env)
				if not env.settings.on then
					return
				end
				for _, repo in ipairs(heiko.await(cmd.repos())) do
					local store = heiko.store.of(repo)
					local request = store.get("request")
					if request ~= nil and answer(store, request, env) then
						-- A changed state draws the interface again, so the
						-- composer says the new one.
						model.answered = (model.answered or 0) + 1
					end
				end
			end,
		},
	},

	-- The composer's suggestion, and the entry's count.
	repo_data = function(_, _, env)
		if not env.settings.on then
			return {}
		end
		local list = heiko.store.get("suggestions") or {}
		local pending = suggest.pending(list, heiko.now(env))
		return {
			composer = suggest.hint(list, heiko.now(env)),
			nav = #pending > 0 and { detail = tostring(#pending) } or nil,
		}
	end,

	repo_nav = { label = "Suggestions", page = "suggested", icon = "arrow" },

	pages = {
		-- What is suggested in the repository, newest first.
		suggested = function(params, _, env)
			local now = heiko.now(env)
			local pending = suggest.pending(heiko.store.get("suggestions") or {}, now)
			local body = {
				ui.title("Suggestions", "What the person may ask next, after the last run here"),
			}
			if #pending == 0 then
				table.insert(body, ui.empty("None yet: they come a few seconds after a run here ends."))
			end
			for _, s in ipairs(pending) do
				table.insert(
					body,
					ui.stack({
						ui.heading(s.title),
						ui.text(s.why ~= "" and s.why or "No reason given."),
						ui.markdown(s.prompt),
						ui.mono(s.model .. " · " .. ago(s.at, now)),
						ui.row({
							ui.button("Put in composer", "heiko.composer", { text = s.prompt }, "primary"),
							ui.button("Start", "heiko.start", { repo = params.repo, text = s.prompt }, "secondary"),
							ui.button("Dismiss", "dismiss", { id = s.id }, "secondary"),
						}),
					})
				)
			end
			table.insert(body, ui.row({ ui.button("Suggest again", "again", {}, "secondary") }))
			return ui.stack(body)
		end,
	},

	actions = {
		-- The person does not want this one: it is not suggested again.
		dismiss = function(args)
			local list, found = suggest.mark(heiko.store.get("suggestions") or {}, tostring(args.id), "dismissed")
			if not found then
				error("No suggestion " .. tostring(args.id) .. " is pending here", 0)
			end
			heiko.store.put("suggestions", list)
			return nil
		end,
		-- A new set, from the last run kept.
		again = function(_, _, env)
			local last = heiko.store.get("last")
			if last == nil then
				error("No run has ended here since suggestions were turned on", 0)
			end
			local request = table.clone(last)
			request.at = heiko.now(env)
			heiko.store.put("request", request)
			return nil
		end,
	},
})
