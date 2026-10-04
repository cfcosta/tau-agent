//! Skills for the demo and its screens: two to load, one with a key tau
//! ignores, and a folder that is not a skill.

use std::{fs, io, path::Path};

/// Writes the demo's skills folder at `dir`.
pub fn seed(dir: &Path) -> io::Result<()> {
    let skill = |name: &str, text: &str| -> io::Result<()> {
        fs::create_dir_all(dir.join(name))?;
        fs::write(dir.join(name).join(crate::SKILL_FILE), text)
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
