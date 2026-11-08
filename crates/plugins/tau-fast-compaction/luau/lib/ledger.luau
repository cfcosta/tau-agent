-- The decisions so far, and what they make of a transcript.
--
-- Ported from `joelhooks/pi-fast-jev-compaction` (`src/ledger.ts`; see
-- THIRD_PARTY_NOTICES.md). A ledger is the decisions by call id; a
-- call's only ever escalates: keep, then drop its result, then drop the
-- call.
local ledger = {}

-- The note that marks a result this plugin cut, and its prefix, which
-- keeps a cut result from being cut again.
ledger.TRUNCATED = "[fast-compaction truncated "

local RANK = { keep = 1, drop_result = 2, drop_call = 3 }

-- A ledger from a list of decisions.
function ledger.of(decisions)
	local by_id = {}
	ledger.merge(by_id, decisions or {})
	return by_id
end

-- Adds `decisions`, each replacing an earlier one only when it
-- escalates it.
function ledger.merge(by_id, decisions)
	for _, decision in ipairs(decisions) do
		local before = by_id[decision.call_id]
		if before == nil or RANK[decision.action] > RANK[before.action] then
			by_id[decision.call_id] = table.clone(decision)
		end
	end
end

-- A copy of the ledger, to stage a pass on.
function ledger.copy(by_id)
	local out = {}
	for id, decision in pairs(by_id) do
		out[id] = table.clone(decision)
	end
	return out
end

-- The decisions, by call id in order.
function ledger.list(by_id)
	local ids = {}
	for id in pairs(by_id) do
		table.insert(ids, id)
	end
	table.sort(ids)
	local out = {}
	for _, id in ipairs(ids) do
		table.insert(out, by_id[id])
	end
	return out
end

function ledger.action(by_id, call_id)
	local decision = by_id[call_id]
	return decision and decision.action or "keep"
end

-- Whether `cut` cuts the result `message`: it is not cut already, and it
-- holds an image or runs past `head + 120` characters.
function ledger.cuts(message, head)
	return string.find(message.text, ledger.TRUNCATED, 1, true) == nil
		and ((message.images or 0) > 0 or utf8.len(message.text) > head + 120)
end

-- Gives every result the ledger would cut, and that has no archive yet,
-- a new archive path from `path`, and returns each path with the text
-- the file must hold: the result's text, whole. Nothing is written.
function ledger.assign_archives(by_id, messages, head, path)
	local archives = {}
	for _, message in ipairs(messages) do
		local decision = message.role == "toolResult" and by_id[message.call] or nil
		if decision ~= nil and decision.action == "drop_result" and decision.archive == nil and ledger.cuts(message, head) then
			decision.archive = path("result")
			table.insert(archives, { path = decision.archive, text = message.text })
		end
	end
	return archives
end

-- A result cut to its first `head` characters and a note naming
-- `archive`, the file that holds its text whole; without an archive, the
-- note says to re-run the tool.
function ledger.cut_text(message, head, archive)
	local text = message.text
	local length = utf8.len(text)
	local kept = string.sub(text, 1, (utf8.offset(text, head + 1) or #text + 1) - 1)
	if head > 0 then
		kept ..= "\n"
	end
	local recovery = archive and ("full result: " .. archive .. " (read or grep it if needed)")
		or "re-run the tool if needed"
	return kept
		.. ledger.TRUNCATED
		.. math.max(length - head, 0)
		.. " chars of this tool result"
		.. (message.ok == false and " (error)" or "")
		.. "; "
		.. recovery
		.. "]"
end

-- What the ledger makes of `messages`, as `tau.rewrite` takes it: the
-- calls it drops, with their results, and the results it cuts to their
-- heads and a note. It keeps every message: tau drops an answer whose
-- calls all went and which has no text left. Everything else stays, in
-- order.
function ledger.apply(by_id, messages, head)
	local keep, drop_calls, texts = {}, {}, {}
	for index, message in ipairs(messages) do
		table.insert(keep, index)
		if message.role == "assistant" then
			for _, call in ipairs(message.calls or {}) do
				if ledger.action(by_id, call.id) == "drop_call" then
					table.insert(drop_calls, call.id)
				end
			end
		elseif message.role == "toolResult" then
			local decision = by_id[message.call]
			if decision ~= nil and decision.action == "drop_result" and ledger.cuts(message, head) then
				table.insert(texts, { index = index, text = ledger.cut_text(message, head, decision.archive) })
			end
		end
	end
	return { keep = keep, drop_calls = array(drop_calls), texts = array(texts) }
end

return ledger
