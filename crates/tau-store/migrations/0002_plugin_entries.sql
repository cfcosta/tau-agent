-- Two more entry kinds (docs/reference/plugins.md):
--   context: a context rewrite; the transcript restarts after it
--   plugin:  a plugin's record, named by `plugin`; never part of the
--            transcript the model sees
-- SQLite cannot change a CHECK constraint, so the table is rebuilt.
CREATE TABLE messages_new (
  run_id     TEXT    NOT NULL REFERENCES runs (id),
  seq        INTEGER NOT NULL,
  kind       TEXT    NOT NULL CHECK (kind IN ('message', 'compaction', 'context', 'plugin')),
  role       TEXT,                          -- user | assistant | toolResult
  plugin     TEXT,                          -- the plugin of a context or plugin entry
  body       TEXT    NOT NULL CHECK (json_valid(body)),
  created_at TEXT    NOT NULL,
  PRIMARY KEY (run_id, seq)
) STRICT;

INSERT INTO messages_new (run_id, seq, kind, role, body, created_at)
SELECT run_id, seq, kind, role, body, created_at FROM messages;

DROP TABLE messages;
ALTER TABLE messages_new RENAME TO messages;
