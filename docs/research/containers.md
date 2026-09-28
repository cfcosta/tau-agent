# Running the coding tools in containers

- Status: research. Nothing here is built.
- Date: 2026-09-28

Today the coding tools in `tau-tools` (`crates/plugins/tools`) run on
the host, as the user. `bash` spawns `/bin/bash -c <command>` in the
tool's root directory. `read`, `write`, `edit`, `grep`, `find` and `ls`
run in-process. A model that runs `rm -rf ~`, reads `~/.ssh`, or sends
the repository to a server can do so.

This document looks at how tau could run those tools, and the code
under edit, inside an isolated environment on Linux and macOS. It ends
with a recommendation and a staged plan.

## What the tools do today

- Every tool takes a `Root` at construction. `Root::resolve` is
  lexical: it normalizes `..`, expands `~`, and accepts absolute paths.
  It is not a security boundary. `read /etc/shadow` or
  `read ~/.aws/credentials` works if the user can read the file.
- `bash` runs the shell in its own process group and kills the group on
  cancel or timeout. A grandchild that calls `setsid` leaves the group
  and survives the kill.
- When output is truncated, `bash` spills the full output to
  `$TMPDIR/tau-bash-<hex>.log` and tells the model the path. The model
  then reads that file with `read`.
- Tools are built once per agent (`Plugin::tools` is read once, by
  `Agent::plugin`). Per-run state lives in `PluginRun`, and a tool sees
  only `ToolCtx { cancel, updates, run }`.

So isolation has two halves. `bash` needs process isolation. The file
tools need real path confinement. A container around `bash` alone does
not stop `read ~/.ssh/id_ed25519`.

## Requirements

- **No root.** Setup and every tool call run as the user. No setuid
  helper that tau ships, no daemon that runs as root.
- **NixOS works.** The host toolchain lives in `/nix/store`, and
  `/bin`, `/usr/bin` hold almost nothing (`/bin/sh`, `/usr/bin/env`).
  `/etc` is mostly symlinks into the store.
- **Fast per call.** A coding run makes hundreds of `bash` calls. The
  per-call overhead should stay in single-digit milliseconds on Linux.
  A per-run setup of up to about a second is acceptable.
- **Filesystem view.** The project directory is read-write at the same
  path it has on the host. The rest of the toolchain is read-only.
  Secrets (`~/.ssh`, `~/.aws`, `~/.config/gh`, keyrings) are not
  visible.
- **Network policy.** At least: none, full, and an allowlist of hosts.
- **Resource limits.** Memory, process count and CPU per run, through
  cgroups v2.
- **Kill the whole tree.** Cancel and timeout must kill every process
  the call started, including daemons and `setsid` children.
- **Same tool behaviour.** The limits, truncation and error strings in
  [tools.md](../reference/tools.md) do not change.

Threat model: the model is not trusted, and neither is code it runs
(build scripts, tests, `npm install`). The user and the host kernel are
trusted. Kernel exploits are out of scope for the namespace options and
in scope for the VM options (gVisor, microVMs, Apple `container`).

## Linux

### Building blocks

All the rootless options on Linux use the same kernel features:

- **User namespace.** The process is root inside and the user outside.
  It unlocks the other namespaces without privileges. Enabled on NixOS
  by default (`security.allowUserNamespaces`). Ubuntu 24.04 and later
  restrict it through AppArmor
  (`kernel.apparmor_restrict_unprivileged_userns=1`): an unconfined
  binary cannot create one unless an AppArmor profile allows it.
  Codex, VS Code and Claude Code have all hit this with `bwrap`.
- **Mount namespace.** A private mount table: bind the project
  read-write, bind the toolchain read-only, put a tmpfs over `$HOME`,
  `/tmp` and `/run`.
- **PID namespace.** The first process is PID 1 of the namespace. When
  it dies, the kernel kills every process in the namespace. This fixes
  the `setsid` escape that process groups have.
- **Network namespace.** An empty namespace has only loopback. Network
  access needs a user-mode stack such as `pasta` (passt) or
  `slirp4netns`, or a proxy reached through a bind-mounted Unix socket.
- **Landlock.** An unprivileged LSM that restricts file access (and,
  since ABI 4, TCP bind and connect by port) for a process and its
  children. ABI 6 (Linux 6.12) scopes abstract Unix sockets and signals.
  ABI 7 came with 6.15, and ABI 8 with Linux 7.0 (all-thread
  enforcement). It needs no namespaces, so it also works where user
  namespaces are blocked.
- **seccomp.** A BPF filter on system calls. Used to deny `ptrace`,
  `mount`, nested `unshare`, `keyctl`, `bpf` and the like.
- **cgroups v2 delegation.** systemd delegates a subtree to each user's
  `user@UID.service`. On the machine this was written on (NixOS 26.11,
  systemd 261), the delegated controllers are `cpu io memory pids`.
  `systemd-run --user --scope -p MemoryMax=… -p TasksMax=…` puts a
  process in a transient scope with limits. The same call is available
  over D-Bus (`StartTransientUnit`). Writing `1` to `cgroup.kill` kills
  every process in the cgroup.

### bubblewrap (`bwrap`)

A small C program (v0.13.0, 2026-09-22) from the Flatpak project. It
builds a sandbox from command-line flags and then execs the command.

- No daemon, no image, no state. One process per call.
- `--unshare-all`, `--ro-bind`, `--bind`, `--tmpfs`, `--dev`, `--proc`,
  `--die-with-parent`, `--new-session`, `--seccomp <fd>`,
  `--cap-drop ALL`. Since 0.13 it uses `mount_setattr` for read-only
  remounts.
- Not setuid on NixOS. A 2026 CVE (CVE-2026-41163) was reported
  against the setuid mode only.
- Used by Flatpak, by Codex CLI on Linux (vendored and built in-tree
  since mid-2026), and by Anthropic's `sandbox-runtime`.
- No cgroups and no network stack of its own. Combine it with
  `systemd-run --user --scope` for limits, and with `pasta` or a proxy
  socket for network.
- It cannot join an existing network namespace by path. A shared
  per-run namespace needs another layer (see "Network policy").

Measured on NixOS 26.11 (kernel 6.18, bwrap 0.12.0): 50 runs of
`bwrap --unshare-all --die-with-parent --new-session --ro-bind / / --dev /dev --proc /proc --tmpfs /tmp --bind $P $P sh -c true`
took 0.26 s, about 5 ms each. The same loop without `bwrap` took about
2 ms each. The project bind was writable and `/etc` was read-only.

The same test showed a leak worth knowing. With `--ro-bind / /` and no
network, `getent hosts example.com` still resolved, through the nscd
socket in `/run/nscd`. A read-only bind does not stop `connect()` on a
pathname Unix socket. The same applies to `ssh-agent`, the D-Bus
session bus, `docker.sock` and `podman.sock` under `/run/user/UID`.
With `--tmpfs /run` the lookup failed. Never bind `/` wholesale: bind
the toolchain paths one by one and put tmpfs over `/run` and `$HOME`.

### Podman rootless

Podman 6.1.2 and 5.8.7 (both 2026-09-16). The GitHub organization moved
from `containers` to `podman-container-tools`.

- Daemonless. Rootless networking uses `pasta` by default since 5.0.
- Runs OCI images, so the toolchain comes from an image, not the host.
  On NixOS the host toolchain can be bind-mounted instead
  (`-v /nix/store:/nix/store:ro`), but then the image adds little.
- Needs `newuidmap`/`newgidmap` (setuid, from shadow) and subuid ranges
  for multi-UID images. A single-UID mapping works without them.
- Limits through cgroups v2 with systemd delegation; `--memory`,
  `--pids-limit`, `--cpus` work rootless when the controllers are
  delegated.
- Per-call cost is high for `podman run` (image store, conmon, network
  setup): hundreds of milliseconds (unverified on this machine; Podman
  is not installed here). The fitting shape is one container per run
  (`podman run -d … sleep infinity`) and `podman exec` per call.
  `exec` still goes through conmon and the runtime, so expect tens of
  milliseconds (unverified).
- Driving it from Rust: the CLI with `--format json`, or the REST API
  on the user socket.

### OCI runtimes: runc, crun, youki

The low-level runtimes that Podman and Docker call. Each takes an OCI
bundle (`config.json` plus a rootfs) and runs it. All three support
rootless mode.

- **runc** v1.5.2 (Go). Re-execs itself through a C shim.
- **crun** 1.30.1 (C). Faster start than runc and usable as a library
  (`libcrun`). Used by default by Podman on most distributions.
- **youki** v0.7.0 (Rust). Its core is the `libcontainer` crate
  (0.7.0, Apache-2.0), which a Rust program can link to create
  containers without a subprocess.

Calling a runtime directly skips images, conmon and networking. tau
would write the `config.json` itself: the same mounts as the `bwrap`
line above, plus `linux.resources` for cgroups. That is more work than
`bwrap` for the same result, and a rootless runtime still needs a
delegated cgroup to set limits. `libcontainer` is the interesting part:
it is the only maintained Rust library that does the full OCI setup,
including cgroups and seccomp. It is pre-1.0.

### systemd-nspawn

Since systemd 256, `systemd-nspawn` can run unprivileged, through
`systemd-nsresourced` and `systemd-mountfsd`. Unprivileged, it accepts
only disk images (`--image=`) that carry Verity data, or polkit asks
interactively; directory trees were proposed later (PR #35685). It fits
long-lived dev environments, not a sandbox per tool call.

### gVisor (`runsc`)

A user-space kernel in Go (release-20260921.0) that intercepts the
sandbox's system calls, so kernel bugs are much harder to reach.
`runsc --rootless --network=none do <cmd>` works rootless, but the docs
say `--rootless` is mainly for `runsc do`: no `create`, no netstack
(network means the host network), and cgroup errors are ignored.
`runsc do` shows the host filesystem read-only with an overlay for
writes. Builds and test suites, heavy on `fork`, `exec` and file I/O,
are the workloads it is slowest at (no numbers measured here).

### Doing it natively from Rust

tau could create the namespaces itself instead of spawning a helper.

| Crate           | Version   | What it does                                                                   | Notes                                                       |
| --------------- | --------- | ------------------------------------------------------------------------------ | ----------------------------------------------------------- |
| `hakoniwa`      | 1.8.0     | namespaces, pivot_root, rlimits, cgroups via systemd, landlock, seccomp, pasta | Library and CLI. LGPL-3.0 with a linking exception. Active. |
| `libcontainer`  | 0.7.0     | youki's OCI runtime as a library                                               | Apache-2.0. Pre-1.0. Heavy for our needs.                   |
| `landlock`      | 0.4.7     | Landlock rulesets, with best-effort ABI fallback                               | The official Rust binding. Small.                           |
| `extrasafe`     | 0.5.1     | seccomp and Landlock for the current process                                   | Last release 2024-04.                                       |
| `birdcage`      | 0.8.1     | Linux namespaces + seccomp, macOS Seatbelt                                     | Repository archived in 2026. Do not adopt.                  |
| `nix`, `rustix` | 0.31, 1.1 | raw `unshare`, `clone3`, `mount_setattr`, `pivot_root`                         | Building blocks for our own code.                           |
| `seccompiler`   | 0.5.0     | seccomp-bpf from Rust (Firecracker)                                            | No libseccomp dependency.                                   |

Doing it natively has two costs:

- **Multi-threading.** `unshare(CLONE_NEWUSER)` fails in a
  multi-threaded process, and tau runs on tokio. The setup has to happen
  in a freshly forked child before `exec`, where only async-signal-safe
  code is allowed. `hakoniwa` and `bwrap` both handle this, which is
  the main reason to use one of them.
- **Surface.** Writing uid maps, mount order, `/proc` and `/dev` setup
  correctly is what `bwrap` has spent ten years on.

Landlock alone is different: `landlock_restrict_self` can be applied in
a `pre_exec` hook of `tokio::process::Command` without namespaces. It
gives file and TCP-port confinement, but no PID namespace, no private
`/tmp`, and nothing for pathname Unix sockets beyond denying access to
the directories that hold them.

### MicroVMs on Linux

`libkrun` (v1.19.5, Rust) runs a process in a small KVM guest with
virtio-fs shares. It needs `/dev/kvm` (usually the `kvm` group, not
root). It is the Linux counterpart of Apple `container`, and a later
option if namespaces are not strong enough.

### NixOS specifics

- The toolchain is `/nix/store` plus `/run/current-system/sw`, the
  user's profile (`~/.nix-profile`, `/etc/profiles/per-user/$USER`) and
  `/etc`. All four must be visible read-only. `PATH` points into them.
- A dev shell (`nix develop`, direnv) puts store paths on `PATH` that
  are already covered by `/nix/store`.
- The Nix daemon socket (`/nix/var/nix/daemon-socket/socket`) lets the
  sandbox build derivations as the daemon. Hide it by default; allow it
  when the run needs `nix build`.
- `/run/nscd` must be hidden, or DNS works with the network off.
- No AppArmor userns restriction on NixOS, and Landlock is in the
  default LSM list (checked on this machine).

### Network policy

Three levels, from simple to strict:

1. **None.** `--unshare-net`. Only loopback. Most edit-build-test loops
   work offline once dependencies are fetched.
2. **Full.** Share the host network namespace. No isolation, but the
   filesystem limits still hold.
3. **Allowlist.** An empty network namespace plus a proxy on the host.
   `sandbox-runtime` bind-mounts a Unix socket into the sandbox, bridges
   it to a local port with `socat`, and sets `HTTP_PROXY`/`HTTPS_PROXY`.
   The host-side proxy allows `CONNECT` only to listed domains. Tools
   that ignore proxy variables get no network, which fails closed.
   `pasta` with a port filter is the alternative for raw TCP.

Filtering by domain inside the proxy is the only practical allowlist.
IP-level rules need root (nftables) or a user-mode stack.

### Resource limits

- Per run, create one transient user scope (D-Bus
  `StartTransientUnit` on the user manager, or `systemd-run --user
--scope`) with `MemoryMax`, `TasksMax` and `CPUQuota`. Each `bash`
  call's sandbox is started inside it, so limits cover the whole run.
  On this machine, 10 `systemd-run --user --scope` calls took 70 ms in
  total.
- On timeout or cancel, killing the sandbox's PID 1 is enough (the PID
  namespace goes with it). `cgroup.kill` on a per-call sub-cgroup is
  the stronger version.
- Without systemd, `setrlimit` (`RLIMIT_AS`, `RLIMIT_NPROC`,
  `RLIMIT_CPU`) is the fallback. It is per process, not per tree.

### How `bash` runs inside

The shape that keeps today's semantics: one sandbox per call, set up
from a per-run policy.

```
bwrap
  --unshare-user --unshare-pid --unshare-ipc --unshare-uts --unshare-cgroup
  [--unshare-net]
  --die-with-parent --new-session --cap-drop ALL
  --ro-bind /nix/store /nix/store
  --ro-bind /run/current-system /run/current-system
  --ro-bind /etc /etc                     # after masking secrets under /etc
  --tmpfs /run --tmpfs /tmp --tmpfs $HOME
  --dev /dev --proc /proc
  --bind $PROJECT $PROJECT
  --bind $SPILL $SPILL                    # bash output logs, see below
  --ro-bind-try ~/.gitconfig ~/.gitconfig # allowlisted dotfiles
  --seccomp 3                             # deny ptrace, mount, keyctl, bpf
  --chdir $PROJECT --clearenv --setenv PATH … --setenv HOME $HOME
  /bin/sh -c <command>
```

- `bash` today spawns a fresh shell per call, so nothing is lost by
  spawning a fresh sandbox per call. Background processes do not
  survive between calls today either (they die with the process
  group, when they do not escape it).
- The whole sandbox is killed by killing `bwrap`: `--die-with-parent`
  kills the namespace's PID 1, and the kernel kills the rest.
- The spill directory must be inside the sandbox view and readable by
  `read`, so it moves from `$TMPDIR` to a per-run directory.

## macOS

### Apple `container` and Containerization

- `container` 1.4.1 (2026-09-09) is the CLI. 1.0.0 shipped on
  2026-06-09, a year after WWDC 2025. It needs Apple silicon and is
  supported on macOS 26 (macOS 15 works with network limits).
- It is built on the Containerization Swift package (0.47.0). Each
  container is its own lightweight VM on Virtualization.framework. The
  guest runs `vminitd`, which exposes a gRPC API over vsock. Apple
  advertises sub-second start times.
- `container-apiserver` is a launch agent (`container system start`)
  that talks XPC to its helpers, one `container-runtime-linux` per
  container. The installer needs an admin password once; containers
  then run as the user.
- Useful flags on `container run`: `--volume`/`--mount …,readonly`
  (virtiofs shares), `--read-only` root, `--tmpfs`, `--cpus`,
  `--memory`, `--user`, `--workdir`, `--no-dns`, `--rosetta` for
  amd64 images, `--read-only-path` and `--masked-path` (experimental).
  `container exec` runs a command in a running container.
- Network: containers attach to a vmnet network. `container network
create --internal` makes a host-only network. A `--network none`
  mode is not in the command reference (unverified whether one
  exists). An allowlist would reuse the proxy approach, with the proxy
  reachable on the host-only network.
- Limits: the VM size is the limit (`--cpus`, `--memory`). Memory freed
  inside the guest is not returned to macOS (partial balloon support),
  so a long run that peaks high keeps that memory until the container
  stops.
- "Container machine" (1.0) is a persistent Linux environment that
  mounts all of `$HOME` read-write by default. A shell, not a sandbox.

Driving it from Rust:

- **CLI.** Spawn `container run -d …` once per run, `container exec`
  per call, `container rm -f` at the end. Output of management commands
  is available as JSON. This is the only interface that is plainly
  meant for other programs. The cost of an `exec` round trip through
  the CLI, the apiserver and vsock is unmeasured here (no Mac
  available; unverified).
- **XPC.** The apiserver's XPC API is what the CLI's Swift client
  library (`ContainerClient`) uses. Reports describe the XPC API as
  frozen at 1.0, but it is not documented for other languages. Calling
  it from Rust means reimplementing the message format. Not
  recommended.
- **Swift bridge.** A small Swift static library over
  `ContainerClient` or Containerization, exposed through `swift-bridge`
  (0.1.59) or a C ABI. It adds Xcode 26 to tau's macOS build. Worth it
  only if CLI latency turns out to matter.

The catch for a coding agent: the code runs on Linux, not macOS. A
Swift, Xcode or macOS-only project cannot be built or tested inside.
For portable projects (Rust, Go, Node, Python) it works, but the
toolchain must come from an image, and the host's `target/` or
`node_modules/` may be for the wrong platform. File access over
virtiofs is slower than native for build-heavy trees (unverified
figure; named volumes are documented as faster than bind mounts).

### Seatbelt (`sandbox-exec`)

- `sandbox-exec -p <profile> <cmd>` applies an SBPL profile (the
  language behind App Sandbox) to a process and its children. The same
  kernel mechanism is reachable through the private `sandbox_init`
  call.
- Deprecated in its man page for years, but still present and used by
  Codex CLI (with a hard-coded `/usr/bin/sandbox-exec` path), Anthropic
  `sandbox-runtime`, and Chrome. It works on macOS 26 according to
  these tools (not tested here). There is an open request on
  `apple/containerization` (#737) asking Apple for a supported
  replacement.
- Deny-by-default file rules (`file-read*`, `file-write*` by subpath),
  `network-outbound` limited to a loopback proxy port, `process-exec`
  and `mach-lookup` rules.
- Start cost is one `exec` plus profile compilation: milliseconds
  (unverified figure).
- No PID or mount namespace. Processes are not hidden, and a child can
  outlive a killed parent unless tau tracks the tree. No resource
  limits beyond `setrlimit`.
- Runs the macOS toolchain natively, which is what most Mac users need
  from a coding agent.

### Virtualization.framework, Lima, Colima, OrbStack

- `objc2-virtualization` (0.3.2) binds Virtualization.framework, so tau
  could boot its own guest with virtiofs and a vsock agent. That means
  owning a kernel, an init, images and networking, which is what
  Containerization already does. Not worth it.
- Lima (v2.2.0; vz and virtiofs by default on macOS), Colima (v0.10.3,
  Lima plus a container runtime) and OrbStack (commercial) each keep one
  long-lived VM shared by everything. Isolation between runs would come
  from containers inside it. They fit a user who already runs one and
  points tau at it, not a default.

## Comparison

| Option                    | Root needed            | Per-call start (warm)             | Isolation                 | FS view control       | Network policy           | Limits           | Host toolchain  | From Rust              | Maturity for this use       |
| ------------------------- | ---------------------- | --------------------------------- | ------------------------- | --------------------- | ------------------------ | ---------------- | --------------- | ---------------------- | --------------------------- |
| bubblewrap                | no                     | ~5 ms (measured)                  | namespaces + seccomp      | full, per path        | none/full; proxy socket  | via user scope   | yes             | spawn                  | high (Flatpak, Codex)       |
| hakoniwa (library)        | no                     | ms (unverified)                   | namespaces + LL + seccomp | full, per path        | none/full/pasta          | built in         | yes             | link                   | medium, one maintainer      |
| Landlock only             | no                     | ~0 (pre_exec)                     | LSM rules only            | allow/deny by path    | TCP ports (ABI 4+)       | rlimits          | yes             | link                   | high kernel, partial fit    |
| Podman rootless           | no (newuidmap setuid)  | tens of ms exec (unverified)      | namespaces + seccomp      | via mounts            | pasta, networks          | built in         | via bind mounts | CLI / REST             | high                        |
| runc / crun / youki       | no                     | ms (unverified)                   | namespaces + seccomp      | via `config.json`     | none unless added        | built in         | via mounts      | spawn / `libcontainer` | high runtimes, work to wire |
| systemd-nspawn (unpriv)   | no (needs nsresourced) | slow (image mount)                | namespaces                | image based           | limited                  | systemd          | no              | spawn                  | low for this use            |
| gVisor rootless           | no                     | 100s of ms (unverified)           | user-space kernel         | host RO + overlay     | none or host only        | ignored rootless | yes             | spawn                  | medium rootless             |
| libkrun microVM           | no (`kvm` group)       | 100s of ms boot (unverified)      | VM                        | virtio-fs shares      | tsi / passt              | VM size          | via shares      | link (`krun-sys`)      | medium                      |
| Apple `container`         | installer only         | sub-second start; exec unmeasured | VM per container          | virtiofs volumes, RO  | vmnet, internal networks | VM size          | no (Linux)      | CLI                    | 1.x, since 2026-06          |
| Seatbelt (`sandbox-exec`) | no                     | ms (unverified)                   | MAC profile               | allow/deny by path    | loopback proxy only      | rlimits          | yes (macOS)     | spawn                  | deprecated, widely used     |
| Lima / Colima / OrbStack  | no                     | shared VM, exec over ssh          | VM shared by all runs     | mounts at VM creation | VM's                     | VM size          | no (Linux)      | CLI                    | high, but user-owned        |

## What other agents do

- **Codex CLI.** Linux: vendored `bwrap` plus a seccomp filter, with
  Landlock as a legacy fallback. macOS: Seatbelt through
  `/usr/bin/sandbox-exec`, network limited to loopback and proxy ports.
  Paths like `.git` stay read-only even inside a writable root.
- **Anthropic `sandbox-runtime`** (v0.0.77). `bwrap` on Linux,
  Seatbelt on macOS, and a host-side HTTP/SOCKS proxy with a domain
  allowlist for both.
- **Docker Sandboxes** (`sbx`). A microVM per agent. Heavier, aimed at
  running a whole agent unattended.

Both lightweight tools chose a process sandbox with the host toolchain
over containers with images. The deciding reason is the one in
"Requirements": the agent has to build and test the user's project
with the user's tools.

## Recommendation

Two families cover the needs:

- **Process sandbox** (default): `bwrap` on Linux, Seatbelt on macOS.
  Host toolchain, read-only host view, project read-write, milliseconds
  per call. This is what Codex and `sandbox-runtime` ship.
- **Image sandbox** (opt-in): a long-lived container per run, one
  `exec` per call. Apple `container` on macOS, Podman (or `libkrun`
  later) on Linux. Stronger isolation, and a clean, reproducible
  toolchain, at the cost of the host tools and a second or so per run.

### API shape

A `Sandbox` lives in `tau-tools`, next to the tools that use it.

```rust
/// What a run's sandbox may touch. Built by the caller.
pub struct Policy {
    /// Read-write, at the same path inside.
    pub writable: Vec<PathBuf>,
    /// Read-only extras beyond the backend's toolchain defaults.
    pub readable: Vec<PathBuf>,
    /// Paths inside `writable` that stay read-only (`.git`, `.jj`).
    pub protected: Vec<PathBuf>,
    pub network: Network,
    /// cgroup limits for the whole run: memory, tasks, CPU.
    pub limits: Limits,
    pub env: Vec<(String, String)>,
}

pub enum Network {
    None,
    Full,
    /// Host names allowed through the proxy.
    Allow(Vec<String>),
}

/// A backend: bubblewrap, Seatbelt, Apple `container`, Podman, or
/// `Host` (no isolation, today's behaviour).
#[async_trait]
pub trait SandboxProvider: Send + Sync + 'static {
    fn name(&self) -> &str;
    /// Sets up one run: the cgroup scope, the proxy, the spill
    /// directory, or the container. Fails closed.
    async fn open(&self, policy: &Policy, run: RunId)
        -> anyhow::Result<Arc<dyn Sandbox>>;
}

#[async_trait]
pub trait Sandbox: Send + Sync {
    /// Starts `argv` in `cwd`, stdin closed, stdout and stderr piped.
    fn spawn(&self, argv: &[OsString], cwd: &Path)
        -> anyhow::Result<SandboxChild>;
    /// Whether a host path is inside the sandbox's view, and writable.
    /// The file tools ask this before touching a path.
    fn access(&self, path: &Path) -> Access;
    /// Where `bash` spills full output. Inside the view.
    fn spill_dir(&self) -> &Path;
    /// Tears the run down: kills what is left, removes the scope,
    /// stops the proxy or the container.
    async fn close(&self);
}

pub struct SandboxChild {
    pub stdout: ChildStdout,
    pub stderr: ChildStderr,
    /// Kills everything the call started (PID 1, `cgroup.kill`, or
    /// `container exec` teardown).
    pub kill: Box<dyn FnOnce() + Send>,
    pub wait: BoxFuture<'static, i32>,
}
```

How it plugs into the existing seams:

- `CodingTools::new(root).sandbox(provider, policy)`. Without
  `.sandbox`, the `Host` provider keeps today's behaviour, so existing
  callers do not change.
- **One sandbox per run.** The tools are shared by every run of an
  agent, so the plugin keeps a map from `RunId` to `Arc<dyn Sandbox>`.
  `Plugin::start` opens the sandbox and inserts it. A failure fails the
  run with `AgentError::Plugin`, which is the fail-closed behaviour we
  want. `PluginRun::finish` closes it and removes it from the map.
  Tools look up `ctx.run`.
- **Forks and sub-agents** use their parent's sandbox (walk
  `PluginCtx::parent`), so a sub-agent cannot widen the policy. A
  sub-agent that needs a different policy gets its own agent and
  plugin.
- **`bash`** calls `sandbox.spawn(["sh", "-c", command], root)` instead
  of `tokio::process::Command`. The accumulator, throttle, idle drain,
  truncation and error strings stay as they are. Cancel and timeout call
  `kill` instead of `killpg`.
- **File tools stay in-process** and check `sandbox.access(path)` after
  resolving symlinks. On Linux, open with `openat2` and
  `RESOLVE_BENEATH` under each allowed root, so a symlink inside the
  project cannot point out of it. On macOS with the image backend, the
  project is a virtiofs share of the same host directory, so host-side
  file tools still see what the container sees. Running the file tools
  in-process keeps `grep` on ripgrep's crates and `edit`'s per-path
  locks working, and avoids a helper binary inside the sandbox.
- A denied path returns the same error string a missing permission
  gives today (`Error code: EACCES`), so the model sees a familiar
  failure.

### Staged plan

1. **Confine the file tools.** Add `Policy` and a `Host` sandbox whose
   `access` enforces `writable`, `readable` and `protected`, with
   symlink-safe resolution. No process isolation yet. This closes
   `read ~/.ssh/…` on every platform and is testable without a kernel
   feature.
2. **Linux process sandbox.** A `Bwrap` provider that builds the
   argument list from `Policy`, with NixOS defaults (`/nix/store`,
   `/run/current-system`, per-user profiles, tmpfs over `/run` and
   `$HOME`), `--unshare-net` for `Network::None`, and a seccomp filter
   built with `seccompiler`. Probe at `open`: if user namespaces are
   blocked (Ubuntu's AppArmor rule), fail with a message that names the
   fix. Tests: the `bash` tests in `crates/plugins/tools/tests/bash.rs`
   run under both providers, plus escape tests (write outside the
   project, `connect` to a socket in `/run`, a `setsid` child that must
   die on timeout).
3. **Limits on Linux.** Open a transient user scope per run over D-Bus
   (`zbus`), start every call inside it, and use `cgroup.kill` on
   cancel. When no user manager is reachable, fall back to rlimits and
   say so in an event.
4. **Seatbelt on macOS.** A `Seatbelt` provider that writes an SBPL
   profile from `Policy` and runs `/usr/bin/sandbox-exec -p`. Track the
   process tree for kills, since there is no PID namespace.
5. **Allowlisted network.** A host-side `CONNECT` proxy in Rust,
   started per run. Linux reaches it through a bind-mounted Unix socket
   and a small bridge; macOS through a loopback port that the profile
   allows. Proxy variables are set in `Policy::env`.
6. **Image sandbox.** An `AppleContainer` provider driven through the
   `container` CLI (run once per run, `exec` per call, `rm -f` in
   `close`), then a `Podman` provider with the same shape on Linux.
   Measure `exec` latency before deciding whether a Swift bridge is
   worth it.
7. **Later, if needed.** Replace `bwrap` with `hakoniwa` or our own
   `clone3` code to drop the external binary, and consider `libkrun`
   for a VM-strength sandbox on Linux.

Stages 1 and 2 give most of the value. Stage 6 is the only one that
needs a Mac with macOS 26 to test.

## Open questions

- Should the default `Policy` allow the network? Offline breaks
  `cargo build` with an empty registry cache and every package install.
  Codex defaults to off; a library might leave the choice to the caller
  and refuse to guess.
- Should `.git` and `.jj` be protected by default? It stops a model
  from rewriting history, but it also stops `jj` and `git commit` when
  a workflow wants them.
- Persistent processes across calls (a dev server started by one `bash`
  call and hit by the next) do not work today and would not work with a
  per-call sandbox. The image backend could allow them, which makes the
  two backends behave differently.
- `bwrap` is an external binary. Vendoring it as Codex does removes the
  runtime dependency but adds a C build step to `tau-tools`.

## Unverified

These could not be checked for this document:

- Per-call latency of `podman exec`, `crun run`, `runsc do`, `libkrun`
  boot, `sandbox-exec`, and `container exec`. Only `bwrap` and
  `systemd-run --user --scope` were measured, on one NixOS machine.
- Whether Apple `container` has a `--network none` mode, and how stable
  the XPC API is for third parties.
- That `sandbox-exec` still works unchanged on macOS 26 (reported by
  users of Codex and `sandbox-runtime`, not tested here).
- virtiofs throughput for build-heavy trees under Apple `container`.
- Whether any Landlock ABI after 8 restricts `connect()` on pathname
  Unix sockets.

## Sources

- bubblewrap releases: <https://github.com/containers/bubblewrap/releases>
- Bubblewrap on the Arch wiki: <https://wiki.archlinux.org/title/Bubblewrap>
- Ubuntu userns restriction and bwrap: <https://github.com/dfaerch/bubblewrap-on-ubuntu>
- Podman releases: <https://github.com/podman-container-tools/podman/releases>
- Podman 5.0 (pasta default): <https://www.redhat.com/en/blog/podman-50-unveiled>
- crun: <https://github.com/containers/crun>
- runc releases: <https://github.com/opencontainers/runc/releases>
- youki and `libcontainer`: <https://github.com/youki-dev/youki>, <https://crates.io/crates/libcontainer>
- systemd v256 release notes: <https://github.com/systemd/systemd/releases/tag/v256>
- Unprivileged directory-tree nspawn PR: <https://github.com/systemd/systemd/pull/35685>
- gVisor rootless: <https://gvisor.dev/docs/user_guide/rootless/>
- gVisor performance guide: <https://gvisor.dev/docs/architecture_guide/performance/>
- Landlock kernel docs: <https://docs.kernel.org/userspace-api/landlock.html>
- Landlock ABI v8 docs patch: <https://ratatoskr.run/linux-man/2026/04/15934246/t>
- hakoniwa: <https://github.com/souk4711/hakoniwa>, <https://crates.io/crates/hakoniwa>
- birdcage (archived): <https://github.com/phylum-dev/birdcage>
- extrasafe: <https://crates.io/crates/extrasafe>
- seccompiler: <https://crates.io/crates/seccompiler>
- libkrun: <https://github.com/containers/libkrun>
- Apple `container`: <https://github.com/apple/container>,
  <https://github.com/apple/container/releases>
- `container` technical overview, volumes, networking, container
  machine, command reference:
  <https://github.com/apple/container/tree/main/docs>
- Containerization: <https://github.com/apple/containerization>
- `container` 1.0 coverage: <https://dev.to/trknhr/apples-container-just-hit-v100-mid>
- sandbox-exec replacement request: <https://github.com/apple/containerization/issues/737>
- Seatbelt for agents: <https://alejandromp.com/development/blog/sandboxing-an-ai-harness-on-macos/>
- Codex sandbox internals: <https://codex.danielvaughan.com/2026/04/08/codex-sandbox-platform-implementation/>
- Anthropic sandbox-runtime: <https://github.com/anthropic-experimental/sandbox-runtime>
- Docker Sandboxes: <https://codex.danielvaughan.com/2026/04/13/docker-sandboxes-codex-cli-microvm-isolation/>
- Lima VM types: <https://lima-vm.io/docs/config/vmtype/>
- OrbStack architecture: <https://docs.orbstack.dev/architecture>
- Apple container vs OrbStack comparison: <https://www.repoflow.io/blog/apple-containers-vs-docker-desktop-vs-orbstack>
