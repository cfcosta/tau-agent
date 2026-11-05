-- tau-todo (ADR 0048): issues a repository keeps for its runs, like
-- beads, and each run's own steps.
local tau = require("tau")
local ui = tau.ui
local steps = require("steps")

local STEP_MARK = { pending = "○", in_progress = "◐", done = "●" }

-- The band's lines for the run's steps.
local function step_lines(list)
	local lines = {}
	for _, step in ipairs(list) do
		local line = STEP_MARK[step.status] .. " " .. step.text
		if step.issue then
			line = line .. " → " .. step.issue
		end
		table.insert(lines, ui.toned(line, step.status == "in_progress" and "info" or "neutral"))
	end
	return lines
end

return tau.plugin {
	name = "tau-todo",
	description = "Issues the repository keeps for its runs, and each run's steps.",
	tools = {
		todo_steps = {
			description = "Replace this run's steps with a new list: your own checklist of how you mean to do what you are doing. Each step has its text and a status, pending, in_progress or done; at most one is in progress. Send the whole list each time.",
			parameters = {
				type = "object",
				properties = {
					steps = {
						type = "array",
						items = {
							type = "object",
							properties = {
								text = { type = "string" },
								status = { type = "string", enum = { "pending", "in_progress", "done" } },
							},
							required = { "text", "status" },
							additionalProperties = false,
						},
					},
				},
				required = { "steps" },
				additionalProperties = false,
			},
			call = function(args, ctx)
				local list, why = steps.check(args.steps)
				if list == nil then
					error(why)
				end
				ctx.state.steps = list
				return { steps = #list, done = steps.done(list) }
			end,
		},
	},
	view = function(state, ctx)
		local list = state.steps or {}
		if #list == 0 then
			return {}
		end
		local current = steps.current(list)
		local head = (current and ("Next: " .. current.text) or "Steps")
			.. ", " .. steps.done(list) .. "/" .. #list
		return {
			band = ui.section("steps", head, step_lines(list), { open = true }),
		}
	end,
}
