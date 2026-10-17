# Architect candidate brief

Each Phase B candidate gets this brief. Fill in the variable parts around it: the task, the Phase A explanation, the candidate's starting point, and its output path. On the main chat it is the `spawn` task of a candidate spawned with `land: false`, and the output path can be where the code will really live, since only the chosen sketch is taken. On a chat, it is the brief you follow yourself for one sketch, with its own scratch path, since you write the sketches in sequence.

---

You are producing one candidate design in the `architect` skill's Phase B. You are working inside that skill's workflow. Produce a candidate design package: the caller's usage, a type sketch, function signatures, a module map, and a rationale shaped by the `architect` skill's `references/rationale-template.md`.

Write only under `{OUTPUT_PATH}`. Do not edit any other file. Bodies stay unimplemented: each one only raises a "not implemented" error, in whatever form the repository's language uses. Commit your package under that path with `vcs_commit` before you end.

Your starting point: {STARTING_POINT}. Commit to it. The other candidates start elsewhere.

Apply the following discipline. The candidates are compared on these axes to pick a base.

- **Caller's usage first.** Write the README-style usage and two or three real call sites before the types, then derive the type sketch from them. The usage is the spec. The two must agree, so reconcile the sketch to the usage, not the reverse.
- **Data structures first.** Get the core types right and the code becomes obvious. Trace each dominant access pattern through the proposed structure. If the answer is "we'll add a map, index or cache later", the structure is wrong.
- **Interface depth.** Compare the capability hidden behind the public interface with the size of that interface. Prefer a simple interface that pulls complexity into the callee, even when the implementation becomes less simple. Do not put transport or wire types in the public API. Parse into domain types behind the interface.
- **Shared state.** If two actors might both write, ask "what happens?" If the answer isn't "nothing", default to per-actor state with a merge at the read boundary, per `principle-separate-before-serializing-shared-state`.
- **Visible boundaries.** Unimplemented bodies, `// TODO` pseudocode for tricky logic, doc comments stating intent and invariants. A reader should be able to trace data from input to output by reading types and signatures alone.
- **Invariants in types.** Hard-to-misuse types beat runtime checks, and runtime checks beat prose comments, per `principle-encode-lessons-in-structure` and `principle-type-system-discipline`.
- **Validate at boundaries, trust types inside**, per `principle-boundary-discipline`. Business logic as pure functions. The shell stays thin.
- **One source of truth per invariant.** Derive instead of syncing.
- **Idempotent state transitions** where they apply, per `principle-make-operations-idempotent`. Ask what happens if the operation runs twice or crashes halfway.
- **Short call chains.** If tracing the flow needs more than three files, flatten the hierarchy, per `principle-laziness-protocol` and `principle-minimize-reader-load`.

You are one of several candidates, each from a different starting point. Produce the best design you can from yours. Don't hedge against the others. The differences between candidates are the signal used to pick a base and graft. Converging on a safe-looking middle defeats the exploration.

End with the arena report contract: `PASS` with what you checked (for example, that every call site in the usage type-checks against the sketch, by building it where you can or by tracing it by hand where you can't), `ISSUES` with each problem you can prove, or `BLOCKED` with what stopped you. Add a short note on the alternatives you considered and rejected.
