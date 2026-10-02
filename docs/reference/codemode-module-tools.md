# Codemode module tools

Codemode owns five fixed nested tools. They are available on `tools` inside a
codemode script and are never declared as separate model tools. Their names,
descriptions, and JSON schemas stay fixed for the run. The list of modules
changes through stored records, not through tool registration.

| Tool             | Arguments                                      | Structured result                                                                                   |
| ---------------- | ---------------------------------------------- | --------------------------------------------------------------------------------------------------- |
| `module_define`  | `{ name, source, signatures?, dependencies? }` | `{ name, version, signatures, dependencies }`                                                       |
| `module_list`    | `{}`                                           | Array of selected `{ name, version, signatures, dependencies }` objects, ordered by name; no source |
| `module_inspect` | `{ name, version? }`                           | `{ name, version, source, signatures, dependencies, tests }`                                        |
| `module_select`  | `{ name, version }`                            | `{ name, version, signatures, dependencies }`                                                       |
| `module_test`    | `{ name, version?, code, tools? }`             | `{ name, version, passed, output, output_truncated, calls, error, error_truncated }`                |

`signatures` and `dependencies` default to empty objects. Dependencies map
module names to exact versions. Definitions are immutable: the version is the
digest of name, source, signatures, and dependencies. Defining the same name
with changed content creates a new version and selects it. `module_select`
accepts only a version already registered under the given name, so selecting
an older version rolls back the selected alias. `module_inspect` uses the
selected version when `version` is omitted; an explicit version inspects that
exact definition. Unknown names and mismatched name/version pairs fail.

`module_define` checks names and quotas, compiles Luau syntax in a fresh safe
VM, and persists the exact `Module::Define` record before returning. It does
not execute the source. No tests attach to a new definition, and
the tool does not promote anything. Source executes only at `require` time.
`module_select` likewise persists a `Module::Select` record before returning.
Both writes remain visible after a later script error; a blocked or invalid
tool call writes no module record. Forks inherit records from their prefix and
then have their own selections. The loader reads current records, so this
works within one script:

```luau
local m = tools.module_define({
    name = "math_extra",
    source = "return { twice = function(n) return n * 2 end }",
    signatures = { twice = "(n: number) -> number" },
})
text(m.version)
text(require("math_extra").twice(21)) -- 42
```

Inspect and roll back with the returned version:

```luau
local old = tools.module_inspect({ name = "math_extra" })
local newer = tools.module_define({
    name = "math_extra",
    source = "return { twice = function(n) return n + n end }",
})
text(newer.version ~= old.version)
text(tools.module_inspect({ name = "math_extra", version = old.version }).source)
tools.module_select({ name = "math_extra", version = old.version })
text(tools.module_list({})[1].version == old.version)
```

Each VM caches an already loaded exact version. The first successful
`require(name)` also pins that name for the rest of the script. If the script
then defines or selects another version, `require(name, new_version)` loads
the new definition; unversioned imports still use the first pin. Mutable values already returned by earlier imports remain in
that VM; a later codemode call starts with a fresh module heap.

See [module tests](codemode-module-tests.md) for fixture syntax, execution
limits, saved reports, and the meaning of a pass.
