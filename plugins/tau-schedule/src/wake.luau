-- A chat's wake-ups (ADR 0039): their names, the message a chat wakes
-- on, and that message read back.
local wake = {}

-- The most a chat may have waiting: one that schedules in a loop stops
-- here.
wake.MAX_WAITING = 100

-- The soonest a wake-up may be, and the furthest, in seconds from now.
wake.SOONEST = 60
wake.FURTHEST = 366 * 86400

-- How long a turn that did not start waits to be tried again.
wake.RETRY = 30

-- Why a sub-agent's `schedule` refuses, in words for the model.
wake.SUB_AGENTS = "A sub-agent ends and lands, so nothing would wake it: "
	.. "say in your answer what should be looked at later, and when, for the "
	.. "chat that started you to schedule."

-- Why repeating is refused, in words for the model.
wake.NO_REPEATS = "A chat wakes itself once: repeating is for agents. "
	.. "Schedule the next time when this one comes, or make an agent (/agent) "
	.. "for work that should keep going on its own."

-- Whether `name` can name a wake-up: lower-case letters, digits, `-` and
-- `_`, starting with a letter, at most 40.
function wake.valid_name(name)
	return #name >= 1 and #name <= 40 and string.match(name, "^[a-z][a-z0-9_-]*$") ~= nil
end

-- Where the chat `run`'s wake-ups are in the store, and the one `name`.
function wake.prefix(run)
	return "wake/" .. run .. "/"
end

function wake.key(run, name)
	return wake.prefix(run) .. name
end

-- Soonest first; by name when two come due at once.
function wake.sort(list)
	table.sort(list, function(a, b)
		if a.at ~= b.at then
			return a.at < b.at
		end
		return a.name < b.name
	end)
	return list
end

-- The message a chat wakes on: one block per wake-up that came due, its
-- name and when (`due`, in the host's local time), then its note.
--
--     [scheduled · recheck · 2026-10-13 09:30]
--     Did #412 merge? If not, look at what its review asks.
function wake.message(woken)
	local blocks = {}
	for _, one in ipairs(woken) do
		table.insert(blocks, "[scheduled · " .. one.name .. " · " .. one.due .. "]\n" .. one.note)
	end
	return table.concat(blocks, "\n\n")
end

-- `text` as the wake-ups of a message tau wrote, `{ name, due, note }`
-- each, if it is one; nil when it is not.
function wake.parse(text)
	local woken = {}
	for line in string.gmatch(text .. "\n", "([^\n]*)\n") do
		local name, due = string.match(line, "^%[scheduled · (.-) · (.-)%]$")
		if name then
			table.insert(woken, { name = name, due = due, note = "" })
		elseif #woken == 0 then
			return nil
		else
			local last = woken[#woken]
			if last.note ~= "" then
				last.note ..= "\n"
			end
			last.note ..= line
		end
	end
	if #woken == 0 then
		return nil
	end
	for _, one in ipairs(woken) do
		one.note = string.gsub(one.note, "%s+$", "")
	end
	return woken
end

return wake
