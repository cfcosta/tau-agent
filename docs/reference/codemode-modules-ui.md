# Codemode modules in the run inspector

The Codemode section of a run inspector lists the module names in that run's
folded records. Each name shows its selected version first and every previous
immutable version below it. A version is the full SHA-256 content digest; the
inspector shows it in full so a selection can be checked exactly. For each
version, the inspector shows its signatures and pinned dependency versions.
The source is behind **Show source** and is rendered only when opened.

Tests stay attached to the exact version they exercised. A row says whether a
controlled test passed or failed. Opening it shows its test source, bounded
fake call expectations, returned output, call report, and error. The UI marks
truncated output and errors. Large fixture and call JSON gets a compact,
bounded preview before rendering. These tests run with supplied fake calls in
an isolated VM. A passing test is evidence for those inputs, not proof of
behavior with live tools or other inputs. See [module tests](codemode-module-tests.md)
for the test VM and record limits.

**Select version** on an earlier version asks the host to select that exact
`{run, name, version}`. The action carries no source or signatures. The host
reads the run's current persisted fork records, verifies that the definition
still exists under that name with matching content, and publishes a
`Module::Select` record. If the version is missing, belongs to another name,
or its saved definition is corrupt, selection is refused and an alert explains
the error. The UI's folded state is a view, not the authority for this action.
Selection does not modify any definition or move its tests to another version.

The inspector uses the same `State.modules` record fold for live updates and
reloaded runs. `State.modules` defaults to an empty library when deserializing
older state. The view, fold, and action request types compile without the
Codemode host feature; the host action itself needs the normal host context.
The inspector has no approval or promotion operation and adds no model-facing
tool.
