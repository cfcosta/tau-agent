-- `heiko.decide`: the decider asked, awaited, with a failure as a value. Only a
-- hook may call these, in a plugin whose `uses` has `decide`.

return function(heiko)
local decide = {}
heiko.decide = decide

-- Asks the decider `request` (`{ state, questions }`, each question saying its
-- `kind`): its answers by question, and what it cost; or nil and why.
-- It awaits: the model may have changed when it returns.
function decide.ask(request)
	local ok, answers, cost = pcall(heiko.await, heiko.cmd.decide.ask(request))
	if not ok then
		return nil, tostring(answers)
	end
	if type(answers) ~= "table" then
		return nil, "The decider gave no answer"
	end
	return answers, type(cost) == "number" and cost or 0
end

-- [`decide.ask`] for questions answered with a probability: the
-- probability of each of `ids`, in their order, and what it cost; or nil
-- and why, when the decider failed or left one out.
function decide.scores(request, ids)
	local answers, cost = decide.ask(request)
	if answers == nil then
		return nil, cost
	end
	local scores = {}
	for at, id in ipairs(ids) do
		local answer = answers[id]
		if type(answer) ~= "table" or type(answer.probability) ~= "number" then
			return nil, "The decider gave no answer to " .. tostring(id)
		end
		scores[at] = answer.probability
	end
	return scores, cost
end

end
