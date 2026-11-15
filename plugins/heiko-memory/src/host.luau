-- What memory reaches of the host, as commands (ADR 0056): `files`, the
-- shape the store and the import read and write through. Each call
-- awaits its command, so it is for a hook's call chain only, and across
-- each the files may have changed.
local heiko = require("heiko")
local cmd = heiko.cmd

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
		return heiko.await(make(...))
	end
end

return host
