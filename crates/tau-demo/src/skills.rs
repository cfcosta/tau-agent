//! The demo's skills (tau-skills, ADR 0050 gap K): a folder of two to
//! load, one with a key tau ignores, and one that is not a skill; and
//! what the Luau plugin draws of it with those tau ships, as the app's
//! sidebar, page and composer have it.

use std::{fs, io, path::Path};

/// Writes the demo's skills folder at `dir`.
pub fn seed(dir: &Path) -> io::Result<()> {
    let skill = |name: &str, text: &str| -> io::Result<()> {
        fs::create_dir_all(dir.join(name))?;
        fs::write(dir.join(name).join("SKILL.md"), text)
    };
    skill(
        "code-review",
        "---\nname: code-review\ndescription: Reviews the current diff for \
         correctness bugs, ranked by severity, with a failing input for \
         each.\n---\n\nRead the diff against trunk. For each bug, say what \
         input fails and how.\n",
    )?;
    skill(
        "release-notes",
        "---\nname: release-notes\ndescription: >\n  Writes release notes \
         from the commits between two tags: groups by kind, leads with what \
         users notice.\nallowed-tools: [bash, read]\n---\n\nRun \
         scripts/commits.sh with the two tags, group what it lists, and \
         lead with what users will notice.\n",
    )?;
    fs::create_dir_all(dir.join("release-notes/scripts"))?;
    fs::write(
        dir.join("release-notes/scripts/commits.sh"),
        "#!/bin/sh\ngit log --oneline \"$1..$2\"\n",
    )?;
    skill(
        "deploy-staging",
        "---\nname: deploy-staging\n---\n\nDeploy.\n",
    )
}

/// What tau-skills keeps of the app with the person's skills in `skills`
/// and tau's data folder at `data`, its page and commands drawn: the Luau
/// plugin run as the app runs it, once, as the demo host starts.
#[allow(
    clippy::disallowed_methods,
    reason = "called as the demo host is made, before any of its work runs (ADR 0028)"
)]
pub fn app_view(
    runtime: &tokio::runtime::Runtime,
    skills: &Path,
    data: &Path,
) -> anyhow::Result<tau_luau_plugins::AppView> {
    use tau_luau_plugins_host::{agent::Active, files::Files, runtime};
    let name = tau_luau_plugins::names::SKILLS;
    // The skill the plugins host ships, where it puts it.
    tau_luau_plugins_host::skill::install(
        &data.join(tau_ui_plugin::SHIPPED_SKILLS),
    )?;
    let files = tau_luau_plugins_host::built_in::files(name)
        .map_err(anyhow::Error::msg)?;
    let loaded = runtime
        .block_on(runtime::load(name, files))
        .map_err(anyhow::Error::msg)?;
    let files = Files::new(&loaded.declaration.uses.files, None, None)
        .with_package(runtime::package_dir(name))
        .with_data(data.to_owned())
        .with_skills(Some(skills.to_owned()));
    let active = Active {
        settings: loaded.declaration.default_settings(),
        loaded,
    };
    let context = runtime::Context {
        run: serde_json::json!({ "kind": "app" }),
        now: serde_json::json!({}),
        settings: active.settings.clone(),
        state: serde_json::json!({}),
        secrets: None,
        store: None,
        job: None,
    };
    let view = runtime.block_on(tau_luau_plugins_host::registry::app_view(
        &active, files, &context,
    ));
    match &view.error {
        Some(error) => anyhow::bail!("tau-skills: {error}"),
        None => Ok(view),
    }
}
