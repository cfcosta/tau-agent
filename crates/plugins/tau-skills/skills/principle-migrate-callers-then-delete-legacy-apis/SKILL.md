---
name: principle-migrate-callers-then-delete-legacy-apis
description: "Load when introducing a new internal API, renaming or reshaping one, while old callers still exist. Migrate every caller and delete the old API in the same wave. No compatibility shims, deprecated aliases, or parallel old and new paths."
---

# Migrate Callers Then Delete Legacy APIs

When you replace an internal API, move every caller to the new one and delete the old one in the same wave. Do not keep the old API alive behind a shim, a deprecated alias, a `_v2` name, a flag, or a parallel old-and-new path.

**Why:** A compatibility layer meant to be temporary becomes permanent. Every reader then sees two ways to do one thing, and the next agent copies whichever example is nearest. Two paths double the tests and the places a bug can hide. The old path rots, because nothing exercises it on purpose.

**Pattern:**
1. Find every caller before you change anything. `grep` for the name and for every string that spells it: config keys, serialized field names, log and error messages, docs, the repository's instructions, comments that point back to it, scripts, and macros. The compiler finds only the callers it compiles.
2. With more than a handful of call sites, write the migration as a codemod: a script, a `sed` over a known file list, or a structural search-and-replace. Load the `principle-build-the-lever` skill. The script also records exactly what changed.
3. Change the API, migrate every caller, and delete the old API. Commit it as one change, or as a short ordered stack in which the last commit leaves only the new path.
4. Update tests to assert the new contract. Delete tests that only protected the old API's implementation details.
5. Spot-check the rename against the real files. Renames silently miss usages in strings, prose, and back-references. After the edit, `grep` for the old name again. Zero hits outside history is the bar.
6. Build and run the repository's tests. The build proves the compiled callers moved. The grep proves the rest.

**When this applies:**
- No external users depend on backward compatibility.
- The project can absorb a coordinated breaking change.
- The new API is part of a simplification or refactor.

When external consumers do exist (a published library, a public HTTP API, a file format on users' disks), version the change and plan the removal with a date. If the repository's instructions say there are no users, take them at their word and delete.

You skipped this when a diff in a codebase with no external consumers adds an alias marked deprecated, a `legacy` module, an `if use_new` branch that keeps the old path in its else, or a comment that says "kept for compatibility".

Distinct from `principle-outcome-oriented-execution`, which keeps a planned multi-phase migration converging on its target without throwaway intermediate states. This principle is the rule for each API inside such a wave: old and new never ship side by side. It pairs with `principle-subtract-before-you-add`, since deleting the old path is the subtraction.
