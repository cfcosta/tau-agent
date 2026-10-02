# Codemode development plan

Status: approved for implementation. Each numbered task is a separate jj
commit, with tests and the applicable reference update. Completed boxes
identify implemented tasks, not planned APIs.

## Scope

Codemode will combine structured tool results, bounded inference, reusable
Luau modules, and large-data artifacts. It will keep fresh VMs and fixed
model-visible tool definitions. It will not modify Tau's runtime.

Direct tools keep their text output, error behavior, and display limits.
Programs receive a separate structured value. A structured error can remain
readable in a script, but its call status must remain an error.

Scratch modules are available immediately within a conversation. Repository
publication requires the person's approval of the exact source version.
Live evaluations require an explicit budget and remain opt-in.

## Tasks

### Foundation

- [x] **01:** Specify the contracts and this implementation sequence.
- [x] **02:** Add deterministic evaluation fixtures and independent oracles.

### M1: Structured results

- [x] **03:** Add bounded `json.encode` and `json.decode` to Luau.
- [x] **04:** Preserve nested-call status separately from its returned value.
- [x] **05:** Add [structured directory entries](reference/codemode-ls.md) to `ls`.
- [x] **06:** Add [structured paths and completeness](reference/codemode-find.md) to `find`.
- [x] **07:** Add [structured matches and context](reference/codemode-grep.md) to `grep`.
- [x] **08:** Add [structured text and ranges](reference/codemode-read.md) to `read`.
- [x] **09:** Add [structured exit status and output metadata](reference/codemode-bash.md) to `bash`.

Tasks 05 through 08 depend on the result contracts, but not on each other.
M1 keeps existing display limits. It does not yet provide complete access
to large inputs.

### M2: Bounded inference

- [x] **10:** Parse [bounded inference requests and validate output schemas](reference/codemode-inference-request.md).
- [x] **11:** Add [shared per-run admission limits](reference/codemode-inference-budget.md).
- [ ] **12:** Admit and report each side-request attempt, including retries.
- [ ] **13:** Add `tools.infer` as a nested tool owned by Codemode.
- [ ] **14:** Store private inference traces and provenance.
- [ ] **15:** Show inference progress, usage, and failures in the card.
- [x] **16:** Add [bounded mapping with ordered settled results](reference/codemode-map.md).
- [ ] **17:** Evaluate structured results and inference against the baseline.

M2 is the first shipping checkpoint. Subsequent milestones depend on its
correctness and evaluation, not on assumed token savings.

### M3: Reusable modules

- [ ] **18:** Add versioned scratch-module records and their fold.
- [ ] **19:** Load registered modules into fresh VMs.
- [ ] **20:** Add definition, listing, and source/signature inspection.
- [ ] **21:** Exercise modules against controlled fake tools.
- [ ] **22:** Add scratch-module inspection and rollback to the UI.
- [ ] **23:** Add repository storage and pinned version selection.
- [ ] **24:** Add approval-controlled repository publication.

### M4: Complete data outside the prompt

- [ ] **25:** Store immutable artifact bytes with quotas and atomic publication.
- [ ] **26:** Add scoped references and bounded range reads.
- [ ] **27:** Let file reads produce complete artifacts.
- [ ] **28:** Connect complete command-output spills to artifacts.
- [ ] **29:** Add artifact inspection and history-aware retention.
- [ ] **30:** Evaluate the complete matrix, including changed inputs.

## Planned contracts

These contracts describe the intended implementation. The implemented API
remains in [codemode.md](reference/codemode.md) and
[tools.md](reference/tools.md).

### JSON

`json.encode(value)` returns compact JSON. `json.decode(text)` returns a
Luau value. Null fields and empty arrays retain their representation.
Numbers use Luau doubles. JSON integers outside the exact range of
±2^53 are refused rather than silently rounded. Encoding follows the
existing value conversion, including non-finite numbers as null.

Each JSON text is at most 1 MiB. Invalid input, cycles, unsupported values,
and excess depth fail through `pcall` with a string diagnostic.

### Structured tool results

Each tool declares an output schema and supplies `ToolOutput::structured`.
Its direct text output does not change. Missing structured values keep the
existing text fallback. Images retain their explicit `image()` behavior.

| Tool   | Program-visible value                                                                                                        |
| ------ | ---------------------------------------------------------------------------------------------------------------------------- |
| `ls`   | Directory, entries, and truncation flag. Each entry preserves its name, kind, size, modification time, and symlink metadata. |
| `find` | Relative paths and truncation flag. Paths are actual records, not lines parsed from the display.                             |
| `grep` | Path, line number, text, and match/context kind for each line, plus truncation flags.                                        |
| `read` | Selected text, requested/returned range, and explicit truncation metadata. Image reads have a tagged variant.                |
| `bash` | Exit code, timeout/cancel state, combined output, truncation metadata, and an optional spill reference.                      |

Completeness applies to the observed operation and its bounds. A limit,
unreadable entry, or byte cut must not imply a complete observation.
Structured results originate from the operation's records, not from parsing
its formatted display. M4 will supply range access to omitted bytes.

### Inference

The initial surface is `tools.infer({ task, context, schema? })`.
`task` is a non-empty instruction. `context` is explicit JSON data, not the
parent transcript. Without a schema, the value is text. With a schema, the
parsed value must satisfy that schema. The returned object contains
`value`, a private `trace_id`, and usage metadata.

An inference exposes no tools. The host selects the model and effort.
The surrounding run retains its instructions, tools, and continuation.
Cancellation of the script cancels its inference requests.

Per-run admission limits bound calls, concurrent attempts, and deadlines.
Retries consume call allowances. Usage is charged once per reported
attempt, including failed attempts. Token and cost thresholds stop new
admissions, but already-running requests can exceed them. A hard provider
output ceiling requires a separate transport capability check.

Traces contain the explicit task, context, output, attempts, and usage.
They remain outside the main conversation. A missing or incomplete trace
is reported explicitly. A model answer never becomes executable code
without a separate program action.

### Modules

A module has a name, immutable version, source, signatures, and test
results. Parameters carry task inputs. Module source must not depend on a
previous VM's captured values. Imports resolve only registered modules.
Module tool calls use the ordinary nested-call pipeline.

Conversation records determine scratch versions, including fork visibility.
A repository run pins its library snapshot at start. A publication request
identifies the exact source and dependency versions. Approval does not
transfer to a replacement source. The model cannot approve publication.

Controlled tests substitute tool responses and make no real side effects.
Passing model-authored tests is evidence, not proof of general correctness.
The evaluation also uses independent cases and changed inputs.

### Artifacts

An artifact contains immutable bytes and a content digest. An opaque
reference identifies the bytes and their authorized scope. A digest alone
does not grant access. Forks can read inherited references, but unrelated
conversations cannot resolve them.

Range reads have explicit byte bounds and text-decoding rules. They never
require the complete artifact in the VM. Storage quotas apply before
publication. Interrupted writes do not publish partial artifacts.
Retention preserves references reachable from retained run history.

## Validation

Every implementation task runs focused tests and Clippy before its jj
commit. `nix fmt` is the formatter. Rust tools run through `nix develop`.
Host-disabled builds protect the phone's dependency boundary.

Hegel properties use valid inputs by construction and independent oracles.
The existing `hegel.toml` supplies test profiles and reproducible CI runs.
Examples cover operational errors and protocol behavior that properties do
not describe conveniently.

The evaluation records task correctness, uncached/cached input tokens,
output tokens, total inference cost, round trips, latency, and repair
attempts. It compares current codemode, improved codemode, hand-written
modules, and agent-written modules. It includes changed inputs and counts
module creation, testing, and maintenance costs.
