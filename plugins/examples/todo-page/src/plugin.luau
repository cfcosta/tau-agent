-- A page with actions: a run's to-do list, which the person keeps from
-- the plugin's page and the sidebar opens.
local tau = require("tau")
local ui = tau.ui

return tau.plugin {
	nav = { label = "To-do", icon = "check" },
	actions = {
		add = function(args, ctx)
			if args.text and args.text ~= "" then
				ctx.state.items = ctx.state.items or {}
				table.insert(ctx.state.items, { text = args.text, done = false })
			end
		end,
		done = function(args, ctx)
			local item = (ctx.state.items or {})[args.at]
			if item then
				item.done = true
			end
		end,
	},
	view = function(state, ctx)
		local items = state.items or {}
		local rows = {}
		local left = 0
		for at, item in ipairs(items) do
			if item.done then
				table.insert(rows, ui.toned(item.text, "neutral"))
			else
				left += 1
				table.insert(rows, ui.row({ ui.text(item.text), ui.button("Done", "done", { at = at }) }))
			end
		end
		table.insert(rows, ui.input("new", { placeholder = "Add a to-do", submit = "add" }))
		return {
			page = ui.stack(rows),
			row = left > 0 and { line = tostring(left) .. " to do" } or nil,
		}
	end,
}
