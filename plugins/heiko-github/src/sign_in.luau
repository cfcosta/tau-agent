-- The sign-in, as the setup step and the Models screen draw it: a way to
-- start it, the code to enter, the token form, or who is signed in.
local heiko = require("heiko")
local ui = heiko.ui
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

-- A line with a state's mark before it: `working`, `met`, `failing`.
local function marked(state, piece)
	return ui.row({ ui.mark(state), piece })
end

-- What happens on GitHub, in order: numbered, ruled rows under a head.
local function on_github()
	local rows = {}
	for n, line in ipairs({
		"Paste the code heiko copied",
		"Pick the repositories heiko may use",
		"Approve heiko's app",
	}) do
		table.insert(rows, ui.row({ ui.square(n), ui.text(line) }))
		table.insert(rows, ui.rule())
	end
	return ui.section("on-github", "On GitHub", rows, { open = true })
end

-- The code to enter, where, and the ways on. Starting the sign-in copied
-- the code and opened GitHub; the job sees the approval by itself.
local function waiting(code)
	return {
		ui.figure(code.code),
		ui.mono(code.url .. " · expires in " .. code.expires),
		marked("working", ui.text("Waiting for you to approve heiko on GitHub…")),
		on_github(),
		ui.row({
			ui.button("Copy code and open GitHub again", "heiko.copy", { text = code.code, url = "https://" .. code.url }, "secondary"),
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
		table.insert(children, marked("working", ui.text("Checking the token…")))
	elseif state.state == "failed" then
		table.insert(children, marked("failing", ui.toned(state.error, "bad")))
	end
	table.insert(children, ui.row({ ui.button("Back to the device code", "token_form", { open = false }, "secondary") }))
	return children
end

-- The sign-in as it stands: a way to start it, the code to enter, the
-- token form, or who is signed in. `done` is the setup step's way on,
-- once signed in; the step also says what happens on GitHub before the
-- sign-in starts, where the Models screen says it in a line.
function sign_in.form(state, token, done)
	if token and state.state ~= "signed_in" then
		return ui.stack(token_form(state))
	end
	local children
	if state.state == "signed_out" then
		children = {
			done and on_github()
				or ui.text("heiko copies a code and opens GitHub; paste it there and approve heiko's app for the repositories you want."),
			ui.row({ ui.button("Sign in with GitHub", "sign_in", {}, "primary"), token_link() }),
		}
	elseif state.state == "starting" then
		children = { marked("working", ui.text("Asking GitHub for a code…")) }
	elseif state.state == "waiting" then
		children = waiting(state.code)
	elseif state.state == "checking" then
		children = { marked("working", ui.text("Checking…")) }
	elseif state.state == "signed_in" then
		local buttons = {}
		if done then
			table.insert(buttons, ui.button("Continue", "heiko.setup_done", {}, "primary"))
		end
		table.insert(
			buttons,
			ui.button(
				"Give heiko's GitHub App access to more repositories",
				"heiko.open_url",
				{ url = github.install_url() },
				"secondary"
			)
		)
		children = { marked("met", ui.text("Signed in as @" .. state.user)), ui.row(buttons) }
	else
		children = {
			marked("failing", ui.toned(state.error or "", "bad")),
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
		detail = "heiko clones the repositories you pick next from GitHub, pushes to them and opens pull requests. The sign-in stays in heiko's secret store, readable only by you.",
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
