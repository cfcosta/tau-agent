# 0059: Plugins ask for a decision, not for a model by name

- Status: accepted. Follows
  [0045](0045-everything-is-a-plugin.md) (services) and
  [0058](0058-heiko-has-a-standard-part.md) (`heiko.decide`).
- Date: 2026-11-15

## Context

Four plugins and codemode's scripts ask a classifier model typed
questions about a state: does a rule hold, is a goal met, which effort
suits a message, which results may be cut. One model answers them
today, TypeSafe's System One, and everything carried its product name:
the crate, the trait, the service, `uses`, the commands, the Luau
global, the settings and the text on screen. A yes/no question was a
`noul`, TypeSafe's word.

Other models can answer the same questions. OpenAI has a decisions
route (`POST /v1/decisions`, with `predicate`, `choice` and `score`
questions). On 2026-11-15 it refused heiko's plan sign-in with 401
`rejected_by_access_enforcement`, `no_matching_rule`, while the same
token ran `/v1/responses`: the route is not open to plan tokens yet.
When it is, or when a plugin answers decisions with the run's own
model, nothing a plugin wrote should have to change.

## Decision

What plugins ask is a **decision**, and what answers is a **decider**.

- The crate is `heiko-decide`; its trait is `Decider`, its error
  `DecideError`, its fake `FakeDecider`. `TypeSafe` is the one decider
  there is, and keeps its name.
- A plugin declares `uses.decide`, and reaches `heiko.decide.ask`,
  `heiko.decide.scores` and `cmd.decide.ask`. A run that has a decider
  says so as `env.run.decide`. Codemode's global is `decide`.
- A yes/no question is a `predicate`, as OpenAI's route has it; `choice`
  and `score` keep their names. TypeSafe's wire still says `noul`, and
  only the types that are sent to it know that.
- The interface says "the decider", and names TypeSafe where a key is
  asked for.

It stays one service with one provider. It becomes a slot when a second
decider exists to fill it: picking between fillers that do not exist is
code nobody can test.

## Consequences

- Nothing in heiko says the old name but what it must: TypeSafe's model
  ids on the wire, and the projects heiko-fast-compaction was ported
  from, which are named as their authors named them.
- A plugin written before this does not load: `uses.jev`, `cmd.jev` and
  `kind = "noul"` are gone, with no alias.
- The saved key's file and `TYPESAFE_API_KEY` are as they were.
