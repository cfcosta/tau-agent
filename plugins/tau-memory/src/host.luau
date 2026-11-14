-- What memory reaches of the host, as commands (ADR 0056): `files`, the
-- shape the store and the import read and write through, and `infer`,
-- answering `{ ok, value, error }`. Each call awaits its command, so it
-- is for a hook's call chain only, and across each the model and the
-- files may have changed.
local tau = require("tau")
local cmd = tau.cmd

local host = {}

host.files = {}
for _, name in ipairs({
	"read",
	"list",
	"image",
	"frontmatter",
	"roots",
	"remote",
	"repositories",
	"repository",
	-- Awaited too, though they answer nothing: the store says when a write
	-- failed, and what comes after a write reads what it wrote.
	"write",
	"remove",
	"rename",
	"mkdir",
}) do
	local make = cmd.files[name]
	host.files[name] = function(...)
		return tau.await(make(...))
	end
end

-- The model's answer to `request`, as `{ ok, value, error }`: with
-- `tools`, `value` is `{ text, calls }`.
function host.infer(request)
	if request.tools ~= nil then
		local ok, text, calls = pcall(tau.await, cmd.infer(request))
		if not ok then
			return { ok = false, error = tostring(text) }
		end
		return { ok = true, value = { text = text, calls = calls } }
	end
	local answer = tau.all({ cmd.infer(request) })[1]
	if not answer.ok then
		return { ok = false, error = answer.error }
	end
	local envelope = (answer.values or {})[1]
	if type(envelope) ~= "table" then
		return { ok = true, value = envelope }
	end
	return envelope
end

return host
