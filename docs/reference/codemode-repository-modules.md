# Codemode repository modules

Repository modules are host-owned immutable versions outside the repository
checkout. The UI configures `Codemode::with_repository` with
`run.repo.dir/codemode-modules`, tau's private per-repository directory. A
custom host can configure the same API with its own private directory. Access
to that directory is within the same-user filesystem trust boundary.

| File                | Contents                                                                                       |
| ------------------- | ---------------------------------------------------------------------------------------------- |
| `versions/<digest>` | Canonical JSON `Definition`; filename is its lowercase SHA-256 content version.                |
| `selected.json`     | Manifest with aliases and approved request receipts; Task 23 plain alias maps remain readable. |
| `.selected.lock`    | Private cross-process lock for manifest updates.                                               |

`versions/<digest>` has a 512 KiB serialized JSON limit. The definition's
unescaped source remains limited to 64 KiB; JSON escaping can expand each
source byte to six bytes. Signatures are limited to 16 KiB of serialized JSON,
and each definition has at most 32 dependencies. The 512 KiB file limit also
covers their bounded representation and the definition envelope. Stage and
read both reject larger version files.

`selected.json` has a 64 KiB file limit, at most 256 aliases, and at most 4096
approval receipts. Alias names
are module identifiers and values are lowercase SHA-256 digests. Activation
checks the alias count and canonical serialized size before replacing the
manifest; reads reject oversized or invalid manifests.

`RepositoryModules::stage` validates a definition and writes a version as a
0600 regular file through a synced temporary file and an atomic, no-clobber
hard link. A repeated stage verifies the existing canonical bytes. A corrupt,
missing, mismatched, or non-regular version fails; it is never replaced or
substituted with another version. Stage does not select the version. Only the
trusted host's approval action changes `selected.json`; it commits an alias
with a receipt under a cross-process lock. There is no script-callable approval
or activation tool. See [module promotion](codemode-module-promotion.md).

At plugin start, the host records one `kind: "repository_pin"` record owned by
the run ID. Its `selected` map fixes the approved aliases, and its `versions`
map contains full definitions, including source, signatures, and exact
dependency closure. It also retains repository dependencies named by inherited
scratch definitions. The pin is verified before use and is available to UI and
phone folds without filesystem access. A resumed run with the same ID reuses
its original pin. A fork or new run takes the currently selected repository
aliases and its own pin; inherited pins owned by other runs do not select
versions. A recognized pin with a missing, empty, or malformed owner fails
validation on resume. A valid foreign-owned pin remains inactive for a new
run. The pin has a separate limit of 256 definitions and 16 MiB of
serialized definitions. Closure capture rejects the 257th distinct version
before reading its file. Repository versions are retained without garbage
collection.

Each script resolves an explicitly selected conversation scratch module first.
Otherwise it uses the run's repository pin. An explicit version resolves that
exact definition from scratch or the pin. Dependency imports use the exact
versions in the definition; missing, corrupt, cyclic, and name-mismatched
dependencies fail visibly. Repository changes after the pin is recorded do not
change definitions imported by that run. `require` accepts module identifiers,
not file paths. Scratch definitions and selections remain conversation scoped
and keep their existing 128-version and 1 MiB limits. A fork's inherited
scratch selection can override its new repository alias; an inherited
repository pin cannot.

`module_list({})` includes the current run's selected repository aliases and
explicit scratch selections, ordered by name. An explicit scratch selection
wins on an alias collision. `module_inspect({name, version?})` resolves a
selected alias or exact version with the same scratch-first lookup as
`require`; it returns source, signatures, exact dependencies, and saved
scratch test reports when present. Neither read tool activates a version or
charges repository definitions to scratch quotas. The run inspector reads the
persisted same-owner pin to show selected aliases and immutable definitions,
including dependency versions, source, and signatures. It does not read the
repository filesystem or offer repository selection controls.
