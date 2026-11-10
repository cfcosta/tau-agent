---
name: verification-skill
description: "Load when a repository has no scripted way to drive its real app and prove behavior, or when asked to create, audit, or update its verify-<app> skill. Create mode writes that skill from the repository; maintain mode drives every feature live and lands proven fixes."
---

# Verification skill

Every serious project needs a scripted way to drive the real app and prove behavior: launch it, use a feature the way a person would, and capture evidence. This skill writes that as a project-local skill named `verify-<app>`, and keeps it honest as the app changes. Other skills point at it whenever they need proof on the real app.

You write the generated skill for the next agent, not for a human. It will be read cold, in the middle of a task, by an agent that has never seen the app.

## Pick the mode

Look for an existing verification skill: a `verify-*` folder in the repository's skills location, or any project skill with launch and drive sections and a feature map.

- None: **create** mode.
- One: **maintain** mode, unless the person asked for a new one.
- Several: `ask` which one to maintain, one option per skill.

## Where the skill lives

Put it in the repository's own skills location: the folder its instructions name, or an existing `.agents/skills/`, `.claude/skills/`, or `skills/` folder at the root. If the repository has none, create `skills/verify-<app>/` at its root. tau reads skills from `~/.agents/skills`, so tell the person they can link it there, with the exact command:

```
ln -s <repository>/skills/verify-<app> ~/.agents/skills/verify-<app>
```

Do not create the link yourself. It is outside the repository.

The generated `SKILL.md` has frontmatter with only `name: verify-<app>` and a `description` that starts with when to load it ("Load when you need to prove a change to <app>'s <web UI, CLI, or API> works...") and names the app and what a person uses it through. Without frontmatter the skill is never listed.

## Create mode

Write the steps below as a numbered checklist in your notes and work through them.

### 1. Interview the repository, not the person

Answer these from the code, and `ask` the person only for what you cannot observe.

- **Interface.** What does a person touch? A web UI, a CLI or TUI, a desktop app, an API, a library? A repository can have several. Pick the primary one and note the rest.
- **Run.** How does the app start locally? Prefer the command its instructions, README, or build files give. Note ports, environment variables, seed data, and auth.
- **Drive.** How can an agent interact with it? Existing harnesses first: end-to-end specs, expect scripts, PTY helpers, endpoints, a debug port, a demo or test mode. Only then pick a generic recipe from `references/harnesses.md` in this skill's folder.
- **Observe.** What evidence can be captured? Screenshots, terminal transcripts, response bodies, logs, exit codes, database state.
- **Isolate.** Can two instances run side by side (ports, data dirs, profiles)? If not, say so in the generated skill. Refusing to drive a shared instance beats corrupting the person's session.

If the checkout does not build or start as it is, fix that first or report it precisely. A skill written against a broken base teaches wrong steps. When an irrelevant missing asset blocks startup (a static dir the API never serves, a sample config), the generated skill may create it, marked as verification scaffolding, and remove it in cleanup.

### 2. Write the skill

Read `references/generated-skill.md` and write the six sections it specifies: Launch, Doctor, Drive, Evidence, Cleanup, Helpers. Ground each one in what the interview found. Leave no placeholders.

For a desktop GUI app on Linux, the generated skill runs the app inside a headless Wayland compositor (for example headless sway, with `grim` for screenshots) and **never falls back to the person's real display**. `references/harnesses.md` has the recipe and the guard that refuses to run when the headless socket is missing. On other platforms with no headless compositor, use the app's own test or demo mode, or stop and tell the person what is missing. Never capture the person's screen.

### 3. Seed the feature map

Create `features/README.md` in the skill's folder, plus one file per user-facing feature you can identify. Start with the top three to five, from routes, commands, menus, or docs. Follow the shape in `references/feature-map-example/` in this skill's folder: a README index with baseline preconditions, driving conventions, and proof rules, then one file per feature. Each feature file answers, from the person's point of view: what the feature is, how to reach it, how to drive it with the harness, and what observable end state proves it works. It has exactly four H2 sections: `Sub-features`, `How to get to it (user POV)`, `Driving it with <harness>`, and `Gotchas`.

If the seed data lacks a precondition a journey needs, the journey reports a failure and the reason goes in that feature's `Gotchas`. Never weaken the journey to make it pass. The map is the repository's maintained verification source. A proof that drives one convenient entry point is incomplete when the map lists others.

### 4. Prove the generated skill

Run its own instructions end to end once: launch, doctor, drive one mapped feature, capture evidence, clean up. After cleanup, confirm the evidence still exists at the named location. A cleanup that eats the proof fails this step. Show a screenshot, if there is one, with `canvas_show`.

Fix what fails, and run the generated cleanup after every failed attempt too, so broken attempts do not strand processes and ports. A generated skill that was never run is a draft. If you proved it in a stand-in for the real target (a container instead of a fresh machine, a local stack instead of a remote one), list every setup step the stand-in did that the target will not. Those steps stay unverified.

### 5. Commit and hand over

Commit the skill with `vcs_commit` (for example `feat(verify): add verify-notes skill with create and search features`). On a chat, finish with `vcs_land`. Tell the person where the skill is, how to link it if it is outside `~/.agents/skills`, and that this skill's maintain mode keeps the map honest. Suggest a cadence only if they ask.

## Maintain mode

A feature map rots the moment the app changes. Read `references/maintain.md` in this skill's folder and follow its pass: index hygiene, a source wave with one read-only sub-agent per feature file, reconcile, a mandatory live pass that drives every feature, triage of each finding as doc drift, harness gap, or product gap, and one landed set of commits of proven corrections. Edit only the verification skill's own folder. Never edit product code in this mode.
