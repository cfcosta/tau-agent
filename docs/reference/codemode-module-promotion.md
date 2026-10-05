# Codemode module promotion

`module_promote({name, version})` is an owned nested codemode tool. Both fields
are required strings. `name` is a scratch module identifier and `version` is
its exact lowercase SHA-256 digest. The result is
`{request_id, name, version, status:"pending"}`. The request is durable before
the tool returns. The tool is present only in a run with
`Codemode::with_repository`; it requires that run's persisted repository pin.
There is no model approval tool. Request creation compiles syntax but never
executes module source or changes repository aliases.

A request stores an opaque UUID, owning run, repository directory scope and its
SHA-256 key, the exact root definition, the complete exact dependency closure,
root test evidence, and a SHA-256 digest of this request content. Scratch
versions resolve before exact versions in the same-owner repository pin. A
missing, mismatched, cyclic, or corrupt dependency fails. A request contains at
most 256 closure definitions and 16 MiB of serialized JSON. A run's inherited
record chain admits at most 128 requests and 32 MiB of request records in total.
The root must be a persisted scratch
definition. Tests are copied only from that root's exact version at request time; later tests
on the same version do not rewrite an earlier request, and tests never follow a
replacement version. The existing per-definition and per-test quotas still
apply.

The run inspector lists Pending, Approved, and Declined requests. The exact
source, signatures, dependency versions, closure, and copied test evidence are
behind a closed disclosure. Pending requests have **Approve** and **Decline**
controls with a 32 px minimum disclosure target. The UI action carries only
`{action:"promote", run, request_id, decision}`. The host reads persisted
`tau-host`, codemode, scratch, and run-owned pin records. It checks the current
repository configuration, request owner and ID, exact root and closure,
content digest, and evidence against the persisted record prefix at request
time. Duplicate or malformed request records, foreign
owners, wrong runs, wrong repositories, and conflicting terminal records fail
before activation. The folded UI state and script store values are display
inputs, not approval authority. A failed action produces a visible alert.

The repository directory contains `versions/<digest>`, `selected.json`, and a
private `.selected.lock`. The manifest is an envelope:

```json
{
  "selected": { "module": "<digest>" },
  "approved_requests": { "<request_id>": { "owner": "<run_id>", "digest": "<request_digest>" } }
}
```

The host can read the Task 23 plain alias map as a legacy manifest. Manifest
reads reject invalid aliases, receipts, more than 4096 receipts, or files over
64 KiB. Approval stages and verifies every closure version first. Under a
cross-process file lock, it then checks the receipt and atomically commits the
root alias and exact receipt in one synced temporary-file rename and directory
sync. Concurrent approvals of different aliases preserve both. An identical
request replay returns already approved without changing any alias, including
when a later request selected a newer version. A conflicting receipt fails.
Receipts are retained; when the quota or byte limit is reached, further
approval fails visibly rather than dropping replay protection. Decline writes
only a terminal record and never stages or activates a version. Repeating the
same decline succeeds without writing another record. Approval and
decline decisions for one repository hold the same cross-process lock through
the Store terminal write.

Approval can commit the filesystem manifest before the Store accepts its
terminal Approved record. The UI reports this partial failure and the pending
request can be retried. Its receipt makes that retry idempotent and prevents it
from reverting a later selection. The manifest and SQLite Store are separate
durability domains; this is not a cross-system transaction. A terminal Approved
record is written only after manifest activation succeeds. A Declined record
cannot later be approved under the same ID. A pending request with an approval
receipt cannot be declined after a failed terminal write; retry approval to
persist the terminal record.

The public Rust repository API stages and reads versions and snapshots pins.
Receipt checks and activation are host-internal operations reached through the
persisted-record validation in `HostHalf::act` (`tau-codemode-host`); a caller-built `Request` is not
an approval grant.

A running or resumed run retains its original immutable repository pin. A fresh
run or fork snapshots the current approved aliases. A fork's inherited scratch
selection can still override a repository alias. Repository approval operates
within the same-user filesystem trust boundary at
`run.repo.dir/codemode-modules`; arbitrary same-user shell or file edits are
outside the trust claim.
