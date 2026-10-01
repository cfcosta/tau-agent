CREATE TABLE runs (
  id            TEXT PRIMARY KEY,           -- uuidv7
  workflow_id   TEXT,                       -- groups the runs of one workflow invocation
  agent         TEXT NOT NULL,              -- Agent::name
  kind          TEXT NOT NULL CHECK (kind IN ('root', 'fork', 'subagent')),
  parent_run_id TEXT REFERENCES runs (id),  -- fork source or calling agent
  fork_seq      INTEGER,                    -- fork: inherit parent messages with seq <= fork_seq
  model         TEXT NOT NULL,
  status        TEXT NOT NULL CHECK (status IN ('running', 'done', 'failed', 'cancelled', 'limit')),
  input_tokens  INTEGER NOT NULL DEFAULT 0,
  output_tokens INTEGER NOT NULL DEFAULT 0,
  cost_usd      REAL    NOT NULL DEFAULT 0,
  turns         INTEGER NOT NULL DEFAULT 0,  -- model turns so far; a resumed run keeps counting
  result        TEXT,                       -- final output; JSON when typed
  error         TEXT,
  title         TEXT,                       -- a model's short name for the run, once written
  created_at    TEXT NOT NULL,
  updated_at    TEXT NOT NULL
) STRICT;
CREATE INDEX runs_by_workflow ON runs (workflow_id, created_at);
CREATE INDEX runs_by_parent   ON runs (parent_run_id);

-- kind: message, or
--   context: a context rewrite by `plugin`; the transcript restarts after it
--   plugin:  a record `plugin` keeps with the run; never in the transcript
CREATE TABLE messages (
  run_id     TEXT    NOT NULL REFERENCES runs (id),
  seq        INTEGER NOT NULL,
  kind       TEXT    NOT NULL CHECK (kind IN ('message', 'context', 'plugin')),
  role       TEXT,                          -- user | assistant | toolResult
  plugin     TEXT,                          -- the plugin of a context or plugin entry
  body       TEXT    NOT NULL CHECK (json_valid(body)),
  created_at TEXT    NOT NULL,
  PRIMARY KEY (run_id, seq)
) STRICT;

-- What each plugin charged to a run, part of the run's own totals: the
-- usage of its side requests (a Jev check, a summary).
CREATE TABLE plugin_costs (
  run_id        TEXT    NOT NULL REFERENCES runs (id),
  plugin        TEXT    NOT NULL,
  input_tokens  INTEGER NOT NULL DEFAULT 0,
  output_tokens INTEGER NOT NULL DEFAULT 0,
  cost_usd      REAL    NOT NULL DEFAULT 0,
  PRIMARY KEY (run_id, plugin)
) STRICT;
