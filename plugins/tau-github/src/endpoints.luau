-- Where GitHub's web and API are, and the GitHub App tau signs in
-- through. A test of the plugin against a fake GitHub replaces this
-- module, and the hosts `uses.network` names, in a copy of the package.
return {
	web = "https://github.com",
	api = "https://api.github.com",
	-- The hosts the two are on, as `uses.network` names them.
	hosts = { "github.com", "api.github.com" },
	-- The GitHub App tau signs in through: its client id, and its name in
	-- GitHub's URLs.
	client_id = "Iv23lisaZLq1FOECQUNe",
	app_slug = "ascend-repository-cfcosta",
}
