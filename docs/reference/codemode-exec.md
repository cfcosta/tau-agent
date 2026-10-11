# Command structured results

`exec_command` and `write_stdin` have an `output_schema`, which describes
the structured result returned to calling tools (codemode). Both answer
with the same object:

```json
{
  "process_id": 3,
  "running": true,
  "exit_code": null,
  "output": "Server ready on :3000",
  "truncated": false,
  "dropped_lines": 0
}
```

- `running` is whether the command is still running when the call
  returns. `process_id` is its id then, to pass to `write_stdin`, and
  `null` once it ended.
- `exit_code` is the exit code once it ended (128 + N for signal N),
  `null` while it runs. A non-zero code is also a failed result
  (`ToolError::Output`) carrying the same structured object.
- `output` is the text the model gets, without the status footer or
  the `Command exited with code N` line: what the process wrote since
  the last call that read it, cut to the last 2000 lines or 50 KiB.
- `truncated` says the cut happened. `dropped_lines` counts lines lost
  unread because the process wrote more than 1 MiB between two reads.

The direct result keeps its text and its terminal `details.term`
([tools.md](tools.md), "exec_command and write_stdin"). Invalid
arguments fail validation before a result exists. Command output is not
published as an artifact: only `read` results are
([retention](codemode-artifact-retention.md)).
