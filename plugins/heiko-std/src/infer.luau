-- `heiko.infer`: the model asked, awaited, with a refusal as a value.
-- Only a hook may call it, in a plugin whose `uses` has `infer`.

return function(heiko)

-- The model's answer to `request`, as `{ ok, value, error, reason }`:
-- `value` when `ok`, else `error`, and the `reason` it was refused for
-- when it was. With `tools` in the request, `value` is `{ text, calls }`
-- once the rounds are done.
function heiko.infer(request)
	if request.tools ~= nil then
		local ok, text, calls = pcall(heiko.await, heiko.cmd.infer(request))
		if not ok then
			return { ok = false, error = tostring(text) }
		end
		return { ok = true, value = { text = text, calls = calls } }
	end
	-- `heiko.all` keeps why an answer was refused; `heiko.await` only its
	-- error.
	local answer = heiko.all({ heiko.cmd.infer(request) })[1]
	if not answer.ok then
		return { ok = false, error = answer.error }
	end
	local envelope = (answer.values or {})[1]
	if type(envelope) ~= "table" then
		return { ok = false, error = "the model gave no answer" }
	end
	return envelope
end

end
