# Codemode module tests

`module_test` accepts only registered scratch roots. To test a repository
root, explicitly inspect its source and dependencies and define a scratch
copy first. A scratch root may depend on exact repository versions captured
in the current run's pin; the fake VM loads only that root's exact dependency
closure. Repository definitions remain outside scratch version and byte
quotas, and test evidence remains attached to the scratch root.

`tools.module_test({ name, version?, code, tools? })` runs `code` as Luau in a
fresh codemode VM. `version` defaults to the selected version. The test should
load its subject with `require(name)` or `require(name, version)`. Before the
assertions run, the test VM loads the target's exact version and its pinned
dependencies once. A failed source or missing dependency fails the test even
if `code` only returns `true`. A bare `require(name)` for the target resolves
to that test version, even when another version is selected. Bare imports of
other module names are unavailable. The saved test
keeps the original `code`; the initialization prelude is not added to it.
Assertions use Luau's `assert`. The tool returns and saves
`{ name, version, passed, output, output_truncated, calls, error, error_truncated }`,
even when an assertion fails. A test passes only when its script succeeds and
every supplied fake call is consumed in order.

```luau
local module = tools.module_define({
    name = "double",
    source = "return function(n) return n * 2 end",
})
local report = tools.module_test({
    name = "double",
    version = module.version,
    code = "assert(require('double', '" .. module.version .. "')(21) == 42)",
})
assert(report.passed)
```

`tools` is an optional ordered array of fake call expectations. Each entry is
`{ name, args, value?, error? }`: `args` is the exact JSON object expected from
the script's call to `tools[name]`, and `value` is the fake readable result.
With `error` alone, the fake raises that message. With both `value` and
`error`, the value remains readable and the call row has error status. Explicit
`value = json.null` is still a readable value, unlike an omitted `value`; an
empty error string still marks the call as failed. The
report's `calls` array records bounded previews of actual attempts, with
`name`, JSON `args`, `args_truncated`, `status`, and `error`. Wrong arguments,
extra calls, and unconsumed expectations fail the test even if its source
catches the error with `pcall`.
After 128 attempts, a final `truncated` row records how many further attempts
were omitted.

```luau
local report = tools.module_test({
    name = "double",
    version = module.version,
    code = "assert(tools.lookup({n=21}).answer == require('double', '" .. module.version .. "')(21))",
    tools = {{ name = "lookup", args = {n = 21}, value = {answer = 42} }},
})
```

The test VM has the normal codemode Luau globals and sandbox, but its host
offers only the supplied fake tool names, the target module's exact version,
and its declared exact dependencies. It has no current-run tools, Jev, infer,
network, files, or processes. A fake called `infer` or `bash` remains a fake.
Test VMs keep the codemode memory and cancellation limits, have a hard 2-second
deadline, and share a cancellation-aware limit of two concurrent VMs. Test
source is limited to 64 KiB, at most 128 fake expectations totaling 1 MiB, and
retained output is limited to 64 KiB. Exceeding the VM's output limit makes the
test fail even if its code catches the error. The report's `output_truncated`
field also records when display output is clipped to fit the serialized report;
that clipping alone does not fail an otherwise passing test. Errors are capped
at 16 KiB of UTF-8 text, with `error_truncated` indicating clipping. Output and
error previews can shrink further to fit the encoded report. Mismatch
diagnostics include a bounded preview of expected arguments. The full encoded
report is capped at 256 KiB before persistence, including JSON escaping.
The library
holds at most 128 test records and 8 MiB of serialized tests.

Each report is stored with its source and fixtures in a `Module::Test` record
bound to the exact version. `module_inspect({name, version?})` returns that
version's `tests` array, each with `name`, `version`, `code`, `tools`, and
`result`. Defining changed content creates a new version with no inherited
tests. A passing generated assertion is evidence for its supplied inputs; an
independent evaluation on changed inputs remains a separate step.
