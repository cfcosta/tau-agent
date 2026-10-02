# Bash structured results

The `bash` tool accepts `{ "command": string, "timeout"?: number }`. Its
`output_schema` describes the structured result returned to calling tools.
Every command outcome uses the same object:

```json
{
  "status": "exited",
  "exit_code": 0,
  "output": "hello\n",
  "truncated": false,
  "truncated_by": null,
  "line_limit_exceeded": false,
  "byte_limit_exceeded": false,
  "total_lines": 1,
  "total_bytes": 6,
  "returned_lines": 1,
  "returned_bytes": 6,
  "last_line_partial": false,
  "spill_path": null,
  "error": null
}
```

`status` is `exited`, `timed_out`, `cancelled`, or `spawn_failed`. An exited
command has an integer `exit_code`, including nonzero and signal-derived codes.
Timeout, cancellation, and spawn failure have a null exit code. `output` is the
Accumulator snapshot: the same bounded command output as the direct text result,
without the spill notice, status message, or `(no output)` placeholder. The
line and byte counts describe decoded output; `spill_path` points to the
raw spill when truncation occurred. `truncated_by` is the existing rolling
snapshot's primary cut label (`lines`, `bytes`, or null). That label can depend
on chunk boundaries when both limits apply. `line_limit_exceeded` and
`byte_limit_exceeded` independently report which full-output bounds were
exceeded. `error` contains the explicit spawn or terminal I/O error when one
occurs.

The direct result retains its existing text and terminal `details.term`. Failed
commands return `ToolError::Output` with the same displayed text and the
structured result. Invalid arguments, including an invalid timeout, fail
argument validation before a command result exists.
