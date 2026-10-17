# 0042: Sub-agents can start fresh, and keep their work aside

- Status: accepted. Amends
  [0015](0015-delegates-fork-their-caller.md): a sub-agent forks its
  caller unless the call says `fresh`. Amends
  [0026](0026-sub-agents-run-detached.md): a sub-agent spawned with
  `land: false` is reported, not landed.
- Amended by [0043](0043-local-repositories-in-place.md): a run's
  head is the Git ref `refs/tau/runs/<run>`, not the bookmark
  `tau/<run>`, so kept work stays under that ref.
- Date: 2026-10-17

## Context

tau ships skills that fan work out to sub-agents for judgment, not
only for throughput: `arena` races candidate designs and has a judge
pick one, `interrogate` sends reviewers with different lenses over a
commit stack, `playbook-eval` compares prompt variants, `reflect` runs
reviewers over a session.

Two things in `spawn` worked against them:

- Every sub-agent forked its caller. A reviewer or a judge saw the
  caller's reasoning, its leanings and, for a judge, often the
  candidates' reports. Its verdict was not independent.
- Every sub-agent's commits landed on its caller. Candidates that lost
  landed too, and on the main chat, which commits on trunk, their
  prototypes went through trunk before the caller deleted them.

## Decision

`spawn` takes two more arguments, both optional:

- `fresh` (default false): the sub-agent starts from its task alone,
  without the caller's conversation (`Agent::as_tool` without
  `forking()`). The task must then say everything the sub-agent needs.
  It costs a fresh prompt: nothing of the caller's cache is reused.
- `land` (default true): with `false`, a sub-agent that finished ends
  as `Ending::Kept`. Its workspace closes, its commits stay under its
  head, the ref `refs/tau/runs/<run>`, beside the caller's, and the
  ending lists its own changes. The host queues it as one that only
  reports, records it as dropped, and tau's report turn names its
  changes. The caller reads them with `vcs_show` and takes files from
  one with `vcs_restore {paths, from}`. A sub-agent that commits
  nothing, as a reviewer, ends kept with no changes.

Kept work lasts for the session. Being recorded as dropped, it is
abandoned by the next start's sweep, like a chat the person dropped.
The operation log keeps it.

## Consequences

- Reviewers and judges run `fresh` with `land: false`: they see only
  their brief and land nothing. Arena candidates run with
  `land: false`; the caller grafts the winner with `vcs_restore` and
  nothing of the losers reaches its stack or trunk.
- A kept sub-agent that failed or was stopped is dropped as before:
  `land` changes only what happens to finished work.
- The skills that fan out (`arena`, `interrogate`, `playbook-eval`,
  `reflect`, `how`, `why`, `verification-skill`, and the principles
  that point at them) say when to use each argument.
- Kept work does not survive a restart. A caller that wants a
  candidate after tau restarts must have taken its files first.
