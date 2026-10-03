# 0023: Main pushes with git; chat pull requests replay onto origin

- Status: accepted. Amends
  [0014](0014-the-model-commits-and-runs-land-as-stacked-diffs.md)
  ("A session merged into trunk is pushed only if the person pushes;
  merging is local") and the rule that tau never needs `git`
  ([jj-lib.md](../research/jj-lib.md), "Fetch and push shell out";
  `docs/reference/vcs.md`, "Projects"). Reference:
  [vcs.md](../reference/vcs.md), "Pushing" and "Pull requests".
- Date: 2026-10-03

## Context

Each GitHub repository has a main chat whose commits move trunk, and
chats land on it ([0015](0015-a-main-chat-per-repository.md)). Nothing
sent trunk back to GitHub: it moved only in tau's clone. Pull requests
re-created a chat's commits through GitHub's REST API, on the parent of
the chat's first commit. Once the main chat had commits GitHub had never
seen, that parent was one of them, and the pull request could not be
made; one from the main chat itself found no commits.

gix fetches and clones without `git`, but has no push that tau can use,
and jj-lib's push runs `git` as a subprocess.

## Decision

### `git` is a runtime dependency, for jj-lib's push alone

- tau runs `git` only through jj-lib's push (`push_updates`). Fetching
  and cloning stay on gix. The model still never gets `git`, and tau-vcs
  spawns nothing itself.
- The Nix package's wrapper puts `git` on `tau-ui`'s `PATH`.
- The GitHub token reaches `git` through the child's environment only:
  `GIT_CONFIG_*` set an empty `credential.helper` (dropping the
  person's own, so none stores the token) and then one that answers with
  `TAU_GIT_TOKEN`. It is never written to a file or put on a command
  line.
- The Git store's `origin` holds only a URL, with no fetch refspec, so
  a push never writes remote-tracking refs that jj would import as
  `<branch>@origin`, which would make pushed commits immutable.

### The main chat pushes trunk

- The main chat's header and sidebar row say how many of trunk's
  commits GitHub lacks (what `<trunk>@git`, as the last fetch or push
  left it, does not have), and offer Push to GitHub.
- A push is a fast-forward of GitHub's branch to trunk, with a lease on
  where the last fetch saw it. The commits go as they are, so their
  commit and change ids stay. A push is under the repository's lock,
  like every other write.
- When GitHub's branch moved since the last fetch, nothing is pushed,
  and the main chat's card offers Fetch and push: the usual update and
  catch-up put the main chat's commits on top of GitHub's, then the push
  goes again. Trunk commits with conflicts are not pushed.

### Chat pull requests replay onto origin

- A chat's pull request carries only the chat's commits: copies of them
  replayed onto GitHub's trunk, as the last fetch saw it, with jj's
  three-way tree merge, in a transaction that is dropped, so the copies
  stay hidden. A copy that would conflict refuses the pull request,
  naming the files (`would conflict on origin/main: a.rs, b.rs`).
- Copies keep each commit's description and author and get change ids
  of their own: when GitHub's branch comes back in a fetch, they do not
  make the chat's commits divergent.
- The copies are pushed with jj-lib to the pull request's branch.
  Keep pushing replays the chat's later commits onto the branch's last
  commit and pushes that, a fast-forward. The REST API only opens the
  pull request, asks for reviews and reads checks.
- The main chat has no pull request: it pushes.

## Alternatives considered

- **A push of our own over gix.** No `git` at all, but tau would own
  the pack negotiation and the receive-pack protocol, which jj-lib and
  `git` already get right.
- **`GIT_ASKPASS` with a script.** The script needs the token, so it is
  either written to disk or read from the environment anyway, and it is
  one more file to keep private.
- **`http.extraHeader` with the token.** Simpler, but git sends the
  header to every request of the session, including redirects to other
  hosts, where a credential helper answers only for the host asked.
- **Pull requests on the chat's own commits.** They stand on the main
  chat's unpushed ones, which the pull request would then carry too.
- **Keep the REST re-creation, based on origin.** It would mean diffing
  and uploading every file again, for what `git` pushes as objects.

## Consequences

- The packaged app needs `git` 2.41 or newer, which jj-lib asks for.
- jj's push writes nothing tau's view would import: pushed branches are
  not bookmarks, and after a trunk push the Git store's branch names the
  pushed commit, as a fetch would leave it.
- Replayed copies stay in the Git store, kept by jj's `refs/jj/keep/*`
  refs, unreachable from any operation.
- A chat that edits files the main chat has not pushed cannot open a
  pull request until main pushes.
