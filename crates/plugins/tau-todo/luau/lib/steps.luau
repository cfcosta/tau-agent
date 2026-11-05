-- A run's steps (ADR 0048): the model's own checklist of how it means to
-- do what it is doing. A step is a line and a status, `pending`,
-- `in_progress` or `done`, with at most one in progress. The list is
-- replaced whole; it lives in the run's state, so a fork goes on with
-- its parent's.

local steps = {}

steps.STATUSES = { pending = true, in_progress = true, done = true }

-- `list` checked: every step a line and a status, one in progress at
-- most. Gives the list, or nil and why not.
function steps.check(list)
	if type(list) ~= "table" then
		return nil, "steps are a list of { text, status }"
	end
	local checked = {}
	local going = 0
	for at, step in ipairs(list) do
		if type(step) ~= "table" or type(step.text) ~= "string" or step.text == "" then
			return nil, "step " .. at .. " needs its text"
		end
		local status = step.status or "pending"
		if not steps.STATUSES[status] then
			return nil, "step " .. at .. " has the status `" .. tostring(status)
				.. "`: it is pending, in_progress or done"
		end
		if status == "in_progress" then
			going += 1
		end
		local kept = { text = step.text, status = status }
		if step.issue ~= nil then
			kept.issue = step.issue
		end
		table.insert(checked, kept)
	end
	if going > 1 then
		return nil, "at most one step is in progress; " .. going .. " are"
	end
	return checked
end

-- How many of `list` are done.
function steps.done(list)
	local n = 0
	for _, step in ipairs(list or {}) do
		if step.status == "done" then
			n += 1
		end
	end
	return n
end

-- The step in progress, if one is.
function steps.current(list)
	for _, step in ipairs(list or {}) do
		if step.status == "in_progress" then
			return step
		end
	end
	return nil
end

return steps
