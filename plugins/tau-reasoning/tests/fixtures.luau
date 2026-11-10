-- The states the demo shows, as the plugin leaves them, and the view it
-- draws from each.
local tau = require("tau")
local t = tau.test

-- gpt-5.5's levels.
local EFFORTS = { "none", "low", "medium", "high", "xhigh" }

-- What Jev charges per million bytes of a request, as tau's fake Jev
-- counts them: one token a byte.
local PRICE_PER_MILLION_INPUT = 0.042

-- The request as Jev gets it: each question as `{ type, instructions,
-- criteria }`.
local function wire(request)
	local questions = {}
	for id, q in pairs(request.questions) do
		local criteria = q.levels
		if q.kind == "choice" then
			criteria = {}
			for name, description in pairs(q.options) do
				if type(name) == "number" then
					criteria[description] = json.null
				else
					criteria[name] = description
				end
			end
		elseif q.kind == "noul" then
			criteria = (q.yes ~= nil or q.no ~= nil) and { ["true"] = q.yes, ["false"] = q.no } or nil
		end
		questions[id] = { type = q.kind, instructions = q.question, criteria = criteria }
	end
	return { state = request.state, questions = questions }
end

-- A Jev `confidence` sure of `probabilities`, one per level, charging
-- for the bytes of the request it gets.
local function jev(probabilities, confidence)
	return {
		ask = function(request)
			local p = {}
			for n, value in ipairs(probabilities) do
				p[tostring(n - 1)] = value
			end
			local cost = #json.encode(wire(request)) * PRICE_PER_MILLION_INPUT / 1000000
			return { effort = { score = 0, confidence = confidence, probabilities = p } }, cost
		end,
	}
end

local function run(id, state, fake)
	return t.run({
		id = id,
		settings = { redecide = false, threshold = 0.7 },
		state = state,
		jev = fake,
	})
end

-- A message coming in on auto, with `prefix` tokens before it.
local function message(task, prefix)
	return {
		input = task,
		instructions = "You are a coding agent.",
		model = "gpt-5.5",
		efforts = EFFORTS,
		prefix_tokens = prefix,
	}
end

local function drawn(r)
	return { state = r.state, view = r:view() }
end

-- High picked as the run starts.
t.fixture("chose", function()
	local r = run("demo", {}, jev({ 0.01, 0.02, 0.09, 0.84, 0.04 }, 0.84))
	r:start(message("Fix the flaky retry test", 0))
	return drawn(r)
end)

-- A later message of a 48k-token conversation keeps medium over Jev's
-- high.
t.fixture("kept", function()
	local earlier = run("earlier", {}, jev({ 0.01, 0.03, 0.9, 0.05, 0.01 }, 0.9))
	earlier:start(message("Design the retry pool", 0))
	earlier:run_end({ stop = "Stop", turns = 3, cost = 0, text = "Done." })
	local r = run("demo", earlier.state, jev({ 0.01, 0.03, 0.11, 0.81, 0.04 }, 0.81))
	r:start(message("Fix the flaky retry test", 48213))
	return drawn(r)
end)
