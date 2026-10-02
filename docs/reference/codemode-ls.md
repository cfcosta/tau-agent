# Structured ls results for Codemode

The ls tool keeps its text output and UI details and also exposes a
machine-readable success value through AgentTool::output_schema. Codemode
returns that value to scripts as the result of tools.ls(...).

    {
      "dir": "/workspace/src",
      "entries": [
        {
          "name": "lib.rs",
          "kind": "file",
          "size": 1280,
          "modified": 1790965400
        },
        {
          "name": "subdir",
          "kind": "dir",
          "items": 2
        }
      ],
      "truncated": false,
      "complete": true
    }

The top-level fields are always present. Entry records use the existing
details::Entry fields: name, kind, size, modified, items, target, and ignored.
Optional values and a false ignored flag follow the existing serialization
rules and can be omitted. kind is dir, file, symlink, or symlink_dir; symlink
metadata follows the target, and a symlink record includes its link target
when available.

truncated is true when the entry limit or the 50 KiB presentation byte limit
cuts the listing. complete is false in those cases and when the directory
scan skips an entry or metadata because of a read error. Finding exactly the
requested number of entries does not hit the entry limit.

The structured entries are collected before text byte truncation and are
bounded by the requested entry limit. Therefore, a byte-truncated text result
can have more structured entries than visible text lines. Names come directly
from filesystem records; newline, colon, and Unicode characters stay within
their entry name. An empty directory and a zero entry limit still return the
structured object. Their direct text output remains (empty directory).
Path errors and top-level directory read errors remain tool errors.

The details field remains the UI-oriented Listing and keeps its existing
display cutoff behavior. The structured object is separate from those details
and from the text sent to the model.

## Verification

crates/plugins/tau-tools/tests/ls.rs checks the output schema with tau-agent's
ArgumentSchema, and compares generated safe filenames against an independent
sorted read of each temporary directory. The property uses 100 cases; the
workspace hegel.toml supplies the fixed-seed CI profile.
