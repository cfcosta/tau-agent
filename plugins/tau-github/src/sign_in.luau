-- The sign-in, as the setup step and the Models screen draw it: a way to
-- start it, the code to enter, the token form, or who is signed in.
local tau = require("tau")
local ui = tau.ui
local github = require("github")

local sign_in = {}

-- What the GitHub App needs of a repository, for the token form too.
sign_in.PERMISSIONS = {
	{ "Contents", "Read and write" },
	{ "Pull requests", "Read and write" },
	{ "Metadata", "Read-only" },
}

local function token_link()
	return ui.button("Use a personal access token", "token_form", { open = true }, "secondary")
end

local function permissions()
	local rows = {}
	for _, row in ipairs(sign_in.PERMISSIONS) do
		table.insert(rows, { ui.text(row[1]), ui.text(row[2]) })
	end
	return ui.table({ "Permission", "Level" }, rows)
end

-- The code to enter, where, and the ways on.
local function waiting(code)
	return {
		ui.title(code.code, code.url .. " · code expires in " .. code.expires .. " · only on the repositories you pick"),
		ui.row({
			ui.button("Copy code and open GitHub", "tau.copy", { text = code.code, url = "https://" .. code.url }, "primary"),
			ui.button("I have approved it", "check", {}, "secondary"),
			token_link(),
		}),
		permissions(),
	}
end

-- The token form: what the token needs, its input, and the way back.
local function token_form(state)
	local children = {
		ui.text(
			"A fine-grained token works in place of the GitHub App. Give it these repository permissions, then paste it here and press Enter."
		),
		permissions(),
		ui.secret(github.SECRET, { placeholder = "github_pat_…" }),
	}
	if state.state == "checking" then
		table.insert(children, ui.status("Checking the token…", nil, "info"))
	elseif state.state == "failed" then
		table.insert(children, ui.toned(state.error, "bad"))
	end
	table.insert(children, ui.row({ ui.button("Back to the device code", "token_form", { open = false }, "secondary") }))
	return children
end

-- The sign-in as it stands: a way to start it, the code to enter, the
-- token form, or who is signed in. `done` is the setup step's way on,
-- once signed in.
function sign_in.form(state, token, done)
	if token and state.state ~= "signed_in" then
		return ui.stack(token_form(state))
	end
	local children
	if state.state == "signed_out" then
		children = {
			ui.text("GitHub shows a code; enter it on GitHub and approve tau's app for the repositories you want."),
			ui.row({ ui.button("Sign in with GitHub", "sign_in", {}, "primary"), token_link() }),
		}
	elseif state.state == "starting" then
		children = { ui.status("Asking GitHub for a code…", nil, "info") }
	elseif state.state == "waiting" then
		children = waiting(state.code)
	elseif state.state == "checking" then
		children = { ui.status("Checking…", nil, "info") }
	elseif state.state == "signed_in" then
		local buttons = {}
		if done then
			table.insert(buttons, ui.button("Continue", "tau.setup_done", {}, "primary"))
		end
		table.insert(
			buttons,
			ui.button(
				"Give tau's GitHub App access to more repositories",
				"tau.open_url",
				{ url = github.install_url() },
				"secondary"
			)
		)
		children = { ui.status("Signed in as @" .. state.user, nil, "good"), ui.row(buttons) }
	else
		children = {
			ui.toned(state.error or "", "bad"),
			ui.row({ ui.button("Try again", "sign_in", {}, "primary"), token_link() }),
		}
	end
	return ui.stack(children)
end

-- The step of first-run setup before the repositories, whose list comes
-- from the sign-in.
function sign_in.step(state, token)
	local heading
	if state.state == "signed_in" then
		heading = "Signed in to GitHub"
	elseif state.state == "waiting" then
		heading = "Enter this code on GitHub"
	else
		heading = "Sign in with GitHub"
	end
	return {
		step = "sign-in",
		at = "before_repositories",
		title = "GitHub",
		heading = heading,
		detail = "tau clones the repositories you pick next from GitHub, pushes to them and opens pull requests. The sign-in stays in tau's secret store, readable only by you.",
		body = sign_in.form(state, token, true),
	}
end

-- The Models screen's section: who is signed in, and the form when no one
-- is.
function sign_in.section(state, token)
	local line, tone, head_button
	if state.state == "signed_in" then
		line, tone = "Signed in as @" .. state.user .. " · clones repositories", "good"
		head_button = ui.button("Sign out", "sign_out", {}, "secondary")
	elseif state.state == "signed_out" then
		line, tone = "Not signed in · for cloning repositories", "neutral"
	else
		line, tone = "Signing in…", "info"
	end
	local head = {
		ui.icon("repo", tone),
		ui.stack({ ui.heading("GitHub"), ui.text(line) }),
		ui.spacer(),
	}
	if head_button then
		table.insert(head, head_button)
	end
	local children = { ui.row(head) }
	if state.state ~= "signed_in" then
		table.insert(children, sign_in.form(state, token, false))
	end
	return ui.card(children)
end

return sign_in
