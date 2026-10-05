-- tau-constitution's own database: each repository's constitution, as
-- tau's UI edits it, and the flagged calls and answers a person looked at.

-- How a repository's checks behave. `repo` names the repository's checkout.
CREATE TABLE constitutions (
  repo       TEXT    PRIMARY KEY,
  on_error   TEXT    NOT NULL CHECK (on_error IN ('allow', 'block')),
  max_holds  INTEGER NOT NULL CHECK (max_holds >= 0),
  updated_at TEXT    NOT NULL
) STRICT;

-- Its rules, in order.
CREATE TABLE constitution_rules (
  repo     TEXT    NOT NULL REFERENCES constitutions (repo) ON DELETE CASCADE,
  id       TEXT    NOT NULL,
  position INTEGER NOT NULL,
  text     TEXT    NOT NULL,
  targets  TEXT    NOT NULL CHECK (json_valid(targets)),  -- JSON array: "tool.field" or "final answer"
  review   REAL    NOT NULL,
  block    REAL    NOT NULL,
  PRIMARY KEY (repo, id),
  UNIQUE (repo, position)
) STRICT;

-- A flagged call or answer a person found fine: off the review queue.
-- `key` is the call's id, or `answer-N`.
CREATE TABLE reviewed (
  run TEXT NOT NULL,
  key TEXT NOT NULL,
  PRIMARY KEY (run, key)
) STRICT;
