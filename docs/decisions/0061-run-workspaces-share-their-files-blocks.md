# 0061: Run workspaces share their files' blocks where the filesystem can

- Status: accepted. Follows
  [0043](0043-local-repositories-in-place.md) (a run's workspace is a
  Git worktree, decision 11).
- Date: 2026-11-16

## Context

Every run, fork and sub-agent gets its own jj workspace under `runs/`,
and jj-lib's `check_out` writes each of its files from the object
store. Two workspaces on the same commit hold two full copies of the
same bytes: heiko's own repository is 1,482 files and about 24 MB, so
each run costs that much again, and a sub-agent fan-out costs it once
per agent.

git-twig (https://github.com/leostera/git-twig) makes `git worktree
add` cheaper on APFS, Btrfs and reflink-enabled XFS. It clones each
file from the source checkout so the clone shares the source's blocks
until one side writes, has Git hash every clone against the commit,
and only then registers the worktree. Its benchmarks show 1.6× to 34×
less disk, and about the same time or slower on small repositories.
It takes that path only when the new worktree is on the source's
`HEAD`, and needs the `git` binary for about ten steps.

Neither fits heiko as it is: a run starts on trunk or a fork's
snapshot, not on the commit some checkout is on, and heiko runs no
`git`. jj-lib, though, never overwrites a file that is already there
when it checks out into an empty directory (it skips it and marks it
for the next snapshot), and its snapshot hashes every file so marked.

## Decision

### A new run workspace takes shared copies of the files it can

When `add_workspace` or `add_workspace_from_snapshot` makes a run's
workspace, before jj checks it out:

1. **Only where the filesystem can.** heiko clones a small probe file
   inside the new workspace's `.jj`. If that fails, nothing below
   happens and the workspace is made exactly as before. On ext4 or
   tmpfs that is every time.
2. **From heiko's own workspaces only.** The sources are the main
   chat's workspace and the other runs' workspaces heiko made, on the
   same device as the new one. Never the person's own checkout (a
   local repository's `default`), and never a workspace the person
   made.
3. **A file only where its source holds the same content.** A path
   qualifies when the commit has a plain file there (not a symlink, a
   conflict or a submodule), the source workspace's tree has the same
   file at that path, and the source's file on disk is a regular file
   with the size and modification time jj recorded at its last
   snapshot. A path with a `.git` or `.jj` component never qualifies.
4. **A clone, never a copy or a link.** `FICLONE` on Linux and
   `fclonefileat` on macOS make a new file that shares the source's
   blocks until either side writes. heiko never falls back to copying
   bytes or to a hard link, so no write in one workspace reaches
   another. The file's mode becomes `0o644` or `0o755` from the
   commit, as jj's own checkout sets it.
5. **jj writes the rest.** `check_out` then writes every file not
   cloned (symlinks, conflicts, new content) and leaves the clones
   where they are.
6. **jj checks the clones.** A snapshot of the new workspace, with no
   new files tracked, reads and hashes each clone. If the tree it finds
   is not the commit's (a source changed between its stat and the
   clone), a second `check_out` rewrites exactly the files that differ
   from the store. The workspace is handed out only when its files are
   the commit's.

A clone that fails because the pair of filesystems cannot share blocks
stops the cloning; any other failure skips that file, and jj writes
it.

### What it costs

- **Disk:** a cloned file takes no new data blocks until it is
  written. This is the point of the change.
- **Time:** the new workspace's clones are hashed once, which the plain
  checkout does not do. For small repositories this is about even;
  the gain is disk, as with git-twig.
- **Where the filesystem cannot share blocks:** one failed probe per
  new workspace.

### What does not change

The workspace's Git worktree, its `HEAD`, its operation and its
working copy are made as before (ADR 0043). Opening a workspace that
exists, forgetting one, and the main chat's own workspace are as
before. Ignored files (build outputs such as `target/`) are never
cloned: they are not in the commit.

## Consequences

- On Btrfs, XFS with reflinks and APFS, a run's workspace costs the
  blocks of its edits, not of the whole tree.
- Nothing changes on filesystems without block sharing. Most Linux
  desktops (ext4) are such.
- The real clone can only be tested on a filesystem that shares
  blocks. The tests run the whole path with a stand-in clone (a plain
  copy) everywhere, and the real one where `HEIKO_TEST_REFLINK_DIR`
  names a directory on such a filesystem.

## Alternatives

- **git-twig itself:** it needs `git`, and its fast path needs the new
  worktree on the source's `HEAD`.
- **Cloning without checking:** trusting size and modification time
  alone is jj's own trust for a snapshot, but a source edited in the
  same millisecond, or with its time set back, would then hand the run
  someone else's bytes. The snapshot check costs one hash per clone.
- **A working copy of our own:** jj-lib's `check_out` already leaves
  existing files alone and its snapshot already checks them, so heiko
  needs no other working-copy implementation.
- **Sharing build outputs:** cloning a source workspace's ignored
  files (such as `target/`) would save far more on this repository,
  but they are not in the commit, cannot be checked against it, and
  would hand a run another run's half-built state. Not now.
