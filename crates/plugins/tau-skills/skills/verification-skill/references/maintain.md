# Maintain mode

An upkeep pass on a `verify-<app>` skill and its feature map. The unit of rigor is the feature, not every sentence. Cover every feature file from source and drive every feature live, without turning every bullet into a separate run.

## Outcomes

Pick one and say which:

- **clean**: every feature got source and live coverage, and nothing needs changing. No commits.
- **changed**: one landed set of commits of proven doc, harness, or map corrections.
- **blocked**: coverage could not finish, or a proven fix could not land safely. Say exactly what blocked it.

## Edit scope

Edit only the verification skill's own folder: its `SKILL.md`, `features/`, and the helpers it owns. Never edit product code in this mode. A behavior the map describes that the app no longer has is either doc drift (fix the map) or a product regression (report it, and do not paper over it in the docs).

## The pass

Write these steps as a numbered checklist in your notes. For a long pass, keep a short decision trail on the canvas: one `canvas_write` item, appended with `canvas_edit`.

0. **Locate the target.** The skill to maintain is the project skill with launch and drive sections and a feature map, usually a `verify-*` folder. With several, `ask` which one. With none, switch to create mode instead of inventing a target.

1. **Index hygiene.** Read `features/README.md` and list the folder with `ls`. Fix missing, extra, duplicate, or dead entries. Keep it light. No generated inventory.

2. **Source wave.** On the main chat, commit any pending work, then `spawn` one sub-agent per feature file, all in one turn (up to 8 at once; with more features, spawn the rest as earlier ones finish). Give each this brief, with its feature file's path:

   > Read `<feature file>`. From source, explain how this user-facing feature works today. Do not edit files, do not commit, and do not launch or drive the app. Return: a feature summary, the source entry points (paths and symbols), likely doc drift with file and line citations or "none", and one concise live-verification recipe.

   Then end the turn. Each finished sub-agent starts a turn of yours with its report. The sub-agents fork your conversation, so they share your view of the map. Ask them to cite source for every drift claim, and check the citations.

   On a chat, where `spawn` is refused, do the same reading yourself, one feature file after another, and write each feature's four-part result in your notes before the next.

3. **Reconcile.** Every feature file has a returned summary. Merge overlapping recipes into as few app states as practical. Spot-check cited drift, and do not re-prove clean claims. Look through recent history (`vcs_log`, `vcs_show`) for user-facing features missing from the map, and require a concrete source path before you call one missing.

4. **Live pass.** Required even when the source looks clean. You do all the driving yourself. Follow the skill's own launch model: one long-lived instance driven one feature after another for servers and UIs, or a fresh isolated session per drive for short-lived CLIs. The skill's Launch section decides. Drive every feature at least once. Hold three invariants the whole pass, whatever fails:
   1. Never drive an instance you have not health-checked since it last did something surprising. Run Doctor before the first drive, on each fresh session where sessions are the unit, and again after any failed drive. Where Doctor cannot see the failure (a wedged UI on a healthy process), reset to a known state or relaunch instead of hoping.
   2. The evidence captured so far survives every cleanup. Check it at its named location. Do not assume it.
   3. Nothing a drive started outlives its usefulness. Clean failed-attempt residue whether the session is stuck, exited, or shared. For a shared instance, clean the residue, not the instance.

   A Doctor failure caused by drift in the skill is drift: fix it within the edit scope and retry once, restarting only what the fix invalidated, before you call the pass blocked. A feature that cannot be reached is `verified-unreachable` only with the concrete prerequisite (auth, an entitlement, an OS, external state) and the route you attempted. If the map omits that prerequisite, that is drift. Re-drive every harness fix live before it lands. Final teardown comes after the last drive of the run, re-proofs included, so nothing outlives the run. Evidence stays.

   For a desktop GUI, every drive runs inside the headless compositor from the skill's Launch section. If it does not come up, the pass is blocked. Never fall back to the person's display. Show screenshots that prove a fix with `canvas_show`.

5. **Triage.** Each finding is one of three kinds:
   - **Doc drift**: the user-POV description is wrong or missing. Fix it.
   - **Harness gap**: the behavior works, but the harness cannot drive it. Fix it. A harness fix follows the helpers rule: scripts executable, invocation shown in the skill body.
   - **Product gap**: the app behavior is broken. Record it for the person, with the drive that shows it. Keep it out of this pass's commits.

6. **Land or stop.** For changed: re-read every changed file, then commit the proven corrections with `vcs_commit`, one commit per feature or per harness fix, with messages such as `docs(verify-notes): search opens with / only outside the editor`. On a chat, finish with `vcs_land`. For clean or blocked: no commits. Report the outcome and the coverage honestly.

Keep the run notes (features covered, unreachable prerequisites, confirmed drift, the outcome) in your reply or on the canvas. Do not commit them.

## Reply

The outcome first. Then, per feature: source covered, live result (`verified`, `verified-unreachable` with the prerequisite, or failed), and any drift fixed. Then the product gaps, each with the drive that shows it.
