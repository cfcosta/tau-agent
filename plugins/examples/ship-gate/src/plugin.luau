-- A lifecycle as a machine: the model asks to ship a change, the person
-- says yes or no from the card, and `release` is blocked until the
-- change it ships was approved.
local tau = require("tau")
local ui = tau.ui
local gate = require("gate")

-- The gate's next state, kept in the run's state.
local function step(ctx, event)
	ctx.state.gate = gate.machine:step(ctx.state.gate, event)
end

return tau.plugin {
	tools = {
		ask_to_ship = {
			description = "Asks the person whether a change may ship. Ask before you release it.",
			parameters = {
				type = "object",
				properties = { change = { type = "string" } },
				required = { "change" },
			},
			call = function(args, ctx)
				step(ctx, { event = "ask", change = args.change })
				return "Asked the person about " .. args.change .. "."
			end,
		},
	},
	actions = {
		approve = function(_, ctx)
			step(ctx, { event = "approve" })
		end,
		refuse = function(args, ctx)
			step(ctx, { event = "refuse", why = args.why })
		end,
	},
	before_tool = function(call, ctx)
		if call.name ~= "release" then
			return nil
		end
		if not gate.allows(ctx.state.gate, call.args.change) then
			return tau.block("Ask with ask_to_ship first, and wait for the person's yes.")
		end
	end,
	after_tool_result = function(result, ctx)
		if result.name == "release" and result.ok then
			step(ctx, { event = "shipped" })
		end
	end,
	view = function(state)
		local s = state.gate
		if not gate.machine:is(s, "waiting") then
			return {}
		end
		return {
			status = ui.status("waiting to ship " .. s.change),
			inspector = ui.row({
				ui.text("Ship " .. s.change .. "?"),
				ui.button("Ship it", "approve"),
				ui.button("Not yet", "refuse", nil, "secondary"),
			}),
		}
	end,
}
