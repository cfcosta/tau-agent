//! `tau-plugin new` (ADR 0053): a package that loads and passes its own
//! tests from the start, a person's or one of tau's own.

use std::path::{Path, PathBuf};

use crate::built_in::LIST;

/// Whether `name` may name a package: lowercase words joined by `-`, and
/// for one of tau's own, beginning `tau-`.
pub fn check_name(name: &str, built_in: bool) -> Result<(), String> {
    let words = !name.is_empty()
        && name.split('-').all(|word| {
            !word.is_empty()
                && word
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
        && name.starts_with(|c: char| c.is_ascii_lowercase());
    if !words {
        return Err(format!(
            "{name:?} is not a package's name: lowercase words joined by `-`"
        ));
    }
    if built_in && !name.starts_with("tau-") {
        return Err(format!("{name}: a plugin tau comes with begins `tau-`"));
    }
    Ok(())
}

/// Writes the package `name` into `dir`: `dir/name`, which must not be
/// there. It blocks.
pub fn write(dir: &Path, name: &str) -> Result<PathBuf, String> {
    let folder = dir.join(name);
    if folder.exists() {
        return Err(format!("{} is there already", folder.display()));
    }
    let tool = format!("{}_greet", name.replace('-', "_"));
    let files = [
        ("tau.toml", manifest(name)),
        ("src/plugin.luau", plugin(name, &tool)),
        ("tests/plugin.luau", tests(&tool)),
        ("README.md", readme(name)),
    ];
    for (path, text) in files {
        let at = folder.join(path);
        if let Some(parent) = at.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("{}: {error}", parent.display()))?;
        }
        std::fs::write(&at, text)
            .map_err(|error| format!("{}: {error}", at.display()))?;
    }
    Ok(folder)
}

/// Adds `name` to the list in `plugins`' `built-in.toml`, last. It
/// blocks.
pub fn list(plugins: &Path, name: &str) -> Result<(), String> {
    let path = plugins.join(LIST);
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    if text.contains(&format!("\"{name}\"")) {
        return Err(format!("{} lists {name} already", path.display()));
    }
    let end = text
        .rfind(']')
        .ok_or_else(|| format!("{} has no list of plugins", path.display()))?;
    let (head, tail) = text.split_at(end);
    let text = format!("{}  \"{name}\",\n{tail}", head.trim_end_matches(' '));
    std::fs::write(&path, text)
        .map_err(|error| format!("{}: {error}", path.display()))
}

/// The folder of tau's own plugins in the checkout `here` is in: the
/// nearest `plugins/` holding a `built-in.toml`, up from `here`.
pub fn checkout_plugins(here: &Path) -> Result<PathBuf, String> {
    here.ancestors()
        .map(|dir| dir.join("plugins"))
        .find(|plugins| plugins.join(LIST).is_file())
        .ok_or_else(|| {
            format!(
                "{} is in no checkout of tau: no plugins/{LIST} above it",
                here.display()
            )
        })
}

fn manifest(name: &str) -> String {
    format!(
        "name = \"{name}\"\nversion = \"0.1.0\"\n\
         description = \"What {name} does, in a line.\"\n"
    )
}

fn plugin(name: &str, tool: &str) -> String {
    format!(
        r#"-- {name}: a tool that greets, counting how many it greeted, with a
-- status line and a button that starts the count again.
local tau = require("tau")
local ui = tau.ui

return tau.plugin {{
	tools = {{
		{tool} = {{
			description = "Greets someone by name.",
			parameters = {{
				type = "object",
				properties = {{ name = {{ type = "string" }} }},
				required = {{ "name" }},
			}},
			call = function(args, ctx)
				ctx.state.greeted = (ctx.state.greeted or 0) + 1
				return "Hello, " .. args.name .. "!"
			end,
		}},
	}},
	actions = {{
		reset = function(_, ctx)
			ctx.state.greeted = 0
		end,
	}},
	view = function(state)
		local greeted = state.greeted or 0
		if greeted == 0 then
			return {{}}
		end
		return {{
			status = ui.status("greeted " .. greeted),
			inspector = ui.row({{ ui.text(greeted .. " greeted"), ui.button("Start again", "reset") }}),
		}}
	end,
}}
"#
    )
}

fn tests(tool: &str) -> String {
    format!(
        r#"-- The tool greets by name and counts it; the button starts the count
-- again.
local tau = require("tau")
local t = tau.test
local g = t.gen

t.case("it greets by name and counts it", function()
	local run = t.run({{}})
	t.equal(run:tool("{tool}", {{ name = "Ada" }}), "Hello, Ada!")
	t.equal(run.state.greeted, 1)
	t.equal(run:view().status.text, "greeted 1")
	run:action("reset", {{}})
	t.equal(run:view(), {{}})
end)

t.property("every greeting names who it greets", function(draw)
	local name = draw(g.text({{ min = 1, max = 20 }}))
	t.equal(t.run({{}}):tool("{tool}", {{ name = name }}), "Hello, " .. name .. "!")
end)
"#
    )
}

fn readme(name: &str) -> String {
    format!(
        "# {name}\n\nWhat {name} does, and for whom.\n\n## Testing\n\n\
         ```sh\ntau-plugin test {name}    # its tests\n\
         tau-plugin check {name}   # what it declares and reaches\n```\n"
    )
}
