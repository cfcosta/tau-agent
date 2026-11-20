-- A lifecycle as a machine: the model asks to ship a change, the person
-- says yes or no from the card, and `release` is blocked until the
-- change it ships was approved.
local heiko = require("heiko")
local ui = heiko.ui
local gate = require("gate")

-- The gate's next state, kept in the run's model.
local function step(model, event)
	model.gate = gate.machine:step(model.gate, event)
end

return heiko.plugin {
	tools = {
		ask_to_ship = {
			kind = "people",
			description = "Asks the person whether a change may ship. Ask before you release it.",
			parameters = {
				type = "object",
				properties = { change = { type = "string" } },
				required = { "change" },
			},
			call = function(args, model)
				step(model, { event = "ask", change = args.change })
				return "Asked the person about " .. args.change .. "."
			end,
			-- The change it asked about, and that the person was asked;
			-- the answer comes from the inspector (ADR 0064).
			subject = "change",
			step = function(input)
				return { subject = input.call.change, receipt = { ui.receipt.stamp("asked", "warn") } }
			end,
		},
	},
	actions = {
		approve = function(_, model)
			step(model, { event = "approve" })
		end,
		refuse = function(args, model)
			step(model, { event = "refuse", why = args.why })
		end,
	},
	before_tool = function(call, model)
		if call.name ~= "release" then
			return nil
		end
		if not gate.allows(model.gate, call.args.change) then
			return heiko.block("Ask with ask_to_ship first, and wait for the person's yes.")
		end
	end,
	after_tool_result = function(result, model)
		if result.name == "release" and result.ok then
			step(model, { event = "shipped" })
		end
	end,
	view = function(model)
		local s = model.gate
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
