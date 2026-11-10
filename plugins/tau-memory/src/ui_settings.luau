-- tau-memory's settings pane (tau-memory's `ui::settings`): whether it
-- saves what each run taught as the run ends, the model it writes notes
-- with then and before compaction drops a conversation, and how hard that
-- model reasons. Search runs locally and needs none. Each control is
-- bound to its setting, and saves it.
local tau = require("tau")
local ui = tau.ui

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

-- A setting's name, and what it does under it.
local function what(name, caption)
	return ui.stack({ ui.heading(name), ui.text(caption) })
end

-- The pane of `settings`.
function M.pane(settings, models)
	return ui.stack({
		ui.card({
			ui.toggle("after_each_run", "Save after each run"),
			ui.text(
				"When a run ends, one request reads it and saves what a later run should know, often nothing. Off, memory keeps what the agent writes and what compaction is about to drop."
			),
			ui.rule(),
			what(
				"Model",
				"Writes notes after each run and before compaction drops the conversation. Search runs locally and needs none."
			),
			ui.choice("family", M.FAMILIES),
			ui.rule(),
			what("Reasoning", "How hard it thinks about what is worth keeping. Auto leaves it to the model."),
			ui.choice("reasoning", M.EFFORTS),
			ui.rule(),
			ui.toggle("follow_chat", "Use the chat's model instead"),
			ui.text("Notes come from whatever model and effort the run is on."),
		}),
		ui.mono(M.runs_as(settings, models)),
	})
end

return M
