-- heiko-memory's settings pane (heiko-memory's `ui::settings`): whether it
-- saves what each run taught as the run ends, the model it writes notes
-- with then and before compaction drops a conversation, and how hard that
-- model reasons. Search runs locally and needs none. Each control is
-- bound to its setting, and saves it.
local heiko = require("heiko")
local ui = heiko.ui

local M = {}

-- The families the pane offers, as the plan names them.
M.FAMILIES = { "sol", "luna", "astra", "terra" }

-- The efforts it offers; `auto` leaves it to the model.
M.EFFORTS = { "auto", "low", "medium", "high" }

-- The model notes are written with and its effort, of `settings` and the
-- plan's `models` (`{ id, family, efforts }`, newest of each family
-- first): nil for the run's own. A family the plan does not offer falls
-- back on the run's model, and an effort the model does not take on the
-- model's default (nil).
function M.model(settings, models)
	if settings.follow_chat then
		return nil
	end
	for _, model in ipairs(models or {}) do
		if model.family == settings.family then
			local effort = settings.reasoning
			if effort == "auto" or not table.find(model.efforts or {}, effort) then
				effort = nil
			end
			return { model = model.id, effort = effort }
		end
	end
	return nil
end

-- What the pane says memory runs as: `Runs as gpt-6-luna · low.`, or the
-- chat's model.
function M.runs_as(settings, models)
	local with = M.model(settings, models)
	if with then
		return "Runs as " .. with.model .. " · " .. (with.effort or "auto") .. "."
	elseif settings.follow_chat then
		return "Runs as each chat's model."
	end
	return "No " .. tostring(settings.family) .. " model on the plan: runs as each chat's model."
end

-- The pane of `settings`.
function M.pane(settings, models)
	return ui.stack({
		ui.toggle("after_each_run", "Save after each run"),
		ui.rule(),
		ui.heading("Model"),
		ui.choice("family", M.FAMILIES),
		ui.rule(),
		ui.heading("Reasoning"),
		ui.choice("reasoning", M.EFFORTS),
		ui.rule(),
		ui.toggle("follow_chat", "Use the chat's model instead"),
		ui.mono(M.runs_as(settings, models)),
	})
end

return M
