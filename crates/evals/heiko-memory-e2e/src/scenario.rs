//! The tasks: small made-up repositories, each hiding one fact that a
//! first run finds by doing its task and a second run needs for another.
//!
//! Every repository is a few bash scripts, so a run needs nothing but
//! bash and coreutils: no network, no toolchain. Each failure is fast.
//!
//! A scenario also has a changed variant: between the runs a commit
//! changes the fact (a variable renamed, a script moved), so what the
//! first run learned is stale. The second run then succeeds only with
//! the new fact, and [`Change::stale`] finds the old one in what it did.

use std::{
    io::Read as _,
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use serde::Serialize;

use crate::E2eError;

/// One task: what the agent is asked, how success is checked, and a
/// solution by hand, which the tests hold the check to.
#[derive(Debug, Clone, Copy)]
pub struct Task {
    pub prompt: &'static str,
    /// A bash script run in the repository afterwards: exit 0 is success.
    pub check: &'static str,
    /// A bash script that does the task, knowing the fact.
    pub solution: &'static str,
}

/// The commit that changes the fact between the runs.
#[derive(Debug, Clone, Copy)]
pub struct Change {
    /// A bash script run in the repository.
    pub script: &'static str,
    /// The paths it changes, as a commit would list them: notes about
    /// them are marked stale.
    pub paths: &'static [&'static str],
    /// A regular expression for the old fact in a command, a path or an
    /// edit: the second run used what it should have re-checked.
    pub stale: &'static str,
    /// The second task done with the new fact.
    pub solution: &'static str,
    /// The fact after the change.
    pub fact: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub struct Scenario {
    pub name: &'static str,
    /// What the first run finds out, in a sentence.
    pub fact: &'static str,
    /// The repository's files, by path.
    pub files: &'static [(&'static str, &'static str)],
    /// A bash script run once the files are written.
    pub init: &'static str,
    /// Paths a fresh checkout would not have, removed between the runs:
    /// build outputs.
    pub outputs: &'static [&'static str],
    pub first: Task,
    pub second: Task,
    pub change: Change,
}

/// Whether the fact changes between the runs.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize,
)]
#[serde(rename_all = "lowercase")]
pub enum Variant {
    /// The fact still holds in the second run.
    Stable,
    /// A commit changed it before the second run.
    Changed,
}

impl Variant {
    pub const ALL: [Variant; 2] = [Variant::Stable, Variant::Changed];

    pub fn name(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Changed => "changed",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|variant| variant.name() == name)
    }
}

impl Scenario {
    /// Writes the repository into `dir`, which must be empty or absent.
    pub fn setup(&self, dir: &Path) -> Result<(), E2eError> {
        std::fs::create_dir_all(dir)?;
        for (path, text) in self.files {
            let path = dir.join(path);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&path, text)
                .map_err(|source| E2eError::Write { path, source })?;
        }
        run_script(dir, self.init)
    }

    /// What lies between the runs: build outputs go, as in a fresh
    /// checkout, and in the changed variant the fact changes.
    pub fn between(
        &self,
        dir: &Path,
        variant: Variant,
    ) -> Result<(), E2eError> {
        for output in self.outputs {
            let path = dir.join(output);
            if path.is_dir() {
                std::fs::remove_dir_all(&path)?;
            } else if path.exists() {
                std::fs::remove_file(&path)?;
            }
        }
        match variant {
            Variant::Stable => Ok(()),
            Variant::Changed => run_script(dir, self.change.script),
        }
    }

    /// Whether the second task is done in `dir`.
    pub fn second_done(&self, dir: &Path) -> Result<bool, E2eError> {
        check(dir, self.second.check)
    }

    /// Whether the first task is done in `dir`.
    pub fn first_done(&self, dir: &Path) -> Result<bool, E2eError> {
        check(dir, self.first.check)
    }

    /// The second task's solution for `variant`.
    pub fn second_solution(&self, variant: Variant) -> &'static str {
        match variant {
            Variant::Stable => self.second.solution,
            Variant::Changed => self.change.solution,
        }
    }
}

/// The scenario called `name`.
pub fn find(name: &str) -> Option<&'static Scenario> {
    SCENARIOS.iter().find(|scenario| scenario.name == name)
}

/// How long a script may take before it counts as failed.
pub const SCRIPT_TIMEOUT: Duration = Duration::from_secs(60);

/// Runs a bash script in `dir`; fails with its output unless it exits 0.
pub fn run_script(dir: &Path, script: &str) -> Result<(), E2eError> {
    let (ok, output) = bash(dir, script)?;
    if !ok {
        return Err(E2eError::Script {
            dir: dir.to_owned(),
            script: script.to_owned(),
            output,
        });
    }
    Ok(())
}

/// Whether a check script exits 0 in `dir`.
pub fn check(dir: &Path, script: &str) -> Result<bool, E2eError> {
    Ok(bash(dir, script)?.0)
}

/// Runs `script` with bash in `dir`, in a bare environment (only `PATH`
/// and `HOME` kept, the C locale), for at most [`SCRIPT_TIMEOUT`].
/// Returns whether it exited 0, and what it printed.
fn bash(dir: &Path, script: &str) -> Result<(bool, String), E2eError> {
    let mut command = Command::new("bash");
    command
        .arg("-c")
        .arg(script)
        .current_dir(dir)
        .env_clear()
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for kept in ["PATH", "HOME"] {
        if let Some(value) = std::env::var_os(kept) {
            command.env(kept, value);
        }
    }
    let mut child = command.spawn().map_err(E2eError::Bash)?;
    // Read both pipes while waiting, so a chatty script cannot block.
    let mut stdout = child.stdout.take().expect("piped");
    let mut stderr = child.stderr.take().expect("piped");
    let out = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stdout.read_to_string(&mut text);
        text
    });
    let err = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        text
    });
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        if started.elapsed() > SCRIPT_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let text = format!(
        "{}{}",
        out.join().unwrap_or_default(),
        err.join().unwrap_or_default()
    );
    Ok((status.is_some_and(|status| status.success()), text))
}

/// Every scenario.
pub const SCENARIOS: &[Scenario] =
    &[TEST_MODE, GENERATED, CONFIG_KEYS, RELEASE, REGISTRY];

/// The test suite refuses to run unless an environment variable is set.
pub const TEST_MODE: Scenario = Scenario {
    name: "test-mode",
    fact: "./test.sh runs only with HARBOR_MODE=test set (ci/env.sh); \
           otherwise it exits 3 and writes no report",
    files: &[
        (
            "README.md",
            "# harbor-text\n\nText helpers in bash: the library is \
             `lib/text.sh`, one test per file in `tests/`. `./test.sh` runs \
             them all and writes `build/report.txt`.\n",
        ),
        (
            "lib/text.sh",
            r##"# Text helpers. Source this file; each function prints its result.

upper() { printf '%s\n' "${1^^}"; }
"##,
        ),
        (
            "tests/test_upper.sh",
            r##"source lib/text.sh
[ "$(upper abc)" = "ABC" ]
"##,
        ),
        (
            "test.sh",
            r##"#!/usr/bin/env bash
# Runs every tests/test_*.sh and writes build/report.txt.
set -u
export LC_ALL=C
cd "$(dirname "$0")"
source ci/env.sh
check_mode || exit 3
mkdir -p build
: > build/report.txt
status=0
for t in tests/test_*.sh; do
  name=$(basename "$t" .sh)
  name=${name#test_}
  if bash "$t"; then
    echo "ok $name" >> build/report.txt
  else
    echo "FAIL $name" >> build/report.txt
    status=1
  fi
done
echo "digest $(cat lib/*.sh tests/test_*.sh | cksum | cut -d' ' -f1)" >> build/report.txt
cat build/report.txt
exit $status
"##,
        ),
        (
            "ci/env.sh",
            r##"# Environment checks shared by the scripts.
check_mode() {
  if [ "${HARBOR_MODE:-}" != "test" ]; then
    echo "test.sh: refusing to run outside test mode (see ci/env.sh)" >&2
    return 1
  fi
}
"##,
        ),
    ],
    init: "chmod +x test.sh",
    outputs: &["build"],
    first: Task {
        prompt: "Add a function `lower` to lib/text.sh that prints its \
                 argument in lowercase, with a test in tests/test_lower.sh, \
                 and run the test suite until it passes.",
        check: r##"set -e
source lib/text.sh
[ "$(lower 'AbC dE')" = "abc de" ]
[ -f tests/test_lower.sh ]
grep -qx 'ok lower' build/report.txt
if grep -q '^FAIL' build/report.txt; then exit 1; fi
[ "$(grep '^digest ' build/report.txt)" = "digest $(cat lib/*.sh tests/test_*.sh | cksum | cut -d' ' -f1)" ]
"##,
        solution: r##"set -e
cat >> lib/text.sh <<'EOF'
lower() { printf '%s\n' "${1,,}"; }
EOF
cat > tests/test_lower.sh <<'EOF'
source lib/text.sh
[ "$(lower AbC)" = "abc" ]
EOF
HARBOR_MODE=test ./test.sh
"##,
    },
    second: Task {
        prompt: "Add a function `title` to lib/text.sh that capitalizes the \
                 first letter of every word (`title 'big red dog'` prints \
                 `Big Red Dog`), with a test in tests/test_title.sh. Run the \
                 test suite so build/report.txt is up to date.",
        check: r##"set -e
source lib/text.sh
[ "$(title 'big red dog')" = "Big Red Dog" ]
[ -f tests/test_title.sh ]
grep -qx 'ok title' build/report.txt
if grep -q '^FAIL' build/report.txt; then exit 1; fi
[ "$(grep '^digest ' build/report.txt)" = "digest $(cat lib/*.sh tests/test_*.sh | cksum | cut -d' ' -f1)" ]
"##,
        solution: r##"set -e
cat >> lib/text.sh <<'EOF'
title() { local out='' word; for word in $1; do out+="${word^} "; done; printf '%s\n' "${out% }"; }
EOF
cat > tests/test_title.sh <<'EOF'
source lib/text.sh
[ "$(title 'big red dog')" = "Big Red Dog" ]
EOF
HARBOR_MODE=test ./test.sh
"##,
    },
    change: Change {
        script: r##"cat > ci/env.sh <<'EOF'
# Environment checks shared by the scripts.
check_mode() {
  if [ "${HARBOR_ENV:-}" != "test" ]; then
    echo "test.sh: refusing to run outside test mode (see ci/env.sh)" >&2
    return 1
  fi
}
EOF
"##,
        paths: &["ci/env.sh"],
        stale: r"\bHARBOR_MODE\b",
        solution: r##"set -e
cat >> lib/text.sh <<'EOF'
title() { local out='' word; for word in $1; do out+="${word^} "; done; printf '%s\n' "${out% }"; }
EOF
cat > tests/test_title.sh <<'EOF'
source lib/text.sh
[ "$(title 'big red dog')" = "Big Red Dog" ]
EOF
HARBOR_ENV=test ./test.sh
"##,
        fact: "./test.sh runs only with HARBOR_ENV=test set",
    },
};

/// The router's code is generated; a hand edit is caught as out of date.
pub const GENERATED: Scenario = Scenario {
    name: "generated",
    fact: "gen/routes.sh is generated from routes.txt by tools/regen.sh; \
           add routes to routes.txt and regenerate, never edit gen/",
    files: &[
        (
            "README.md",
            "# harbor-routes\n\nA tiny router: `./serve.sh PATH` prints the \
             response for PATH.\n",
        ),
        ("routes.txt", "# path response\n/ping pong\n"),
        (
            "tools/regen.sh",
            r##"#!/usr/bin/env bash
# Builds gen/routes.sh from routes.txt. With --check, fails if it is out of date.
set -eu
cd "$(dirname "$0")/.."
render() {
  echo "# GENERATED by tools/regen.sh from routes.txt. Do not edit: it is overwritten."
  echo 'route() {'
  echo '  case "$1" in'
  while read -r path response; do
    case "$path" in ''|'#'*) continue ;; esac
    printf "    %s) echo '%s' ;;\n" "$path" "$response"
  done < routes.txt
  echo '    *) echo "404 $1"; return 1 ;;'
  echo '  esac'
  echo '}'
}
if [ "${1:-}" = --check ]; then
  render | cmp -s - gen/routes.sh || { echo "gen/routes.sh is out of date" >&2; exit 1; }
else
  mkdir -p gen
  render > gen/routes.sh
fi
"##,
        ),
        (
            "serve.sh",
            r##"#!/usr/bin/env bash
# Prints the response for the path in $1.
cd "$(dirname "$0")"
source gen/routes.sh
route "${1:-/}"
"##,
        ),
    ],
    init: "chmod +x serve.sh tools/regen.sh && tools/regen.sh",
    outputs: &[],
    first: Task {
        prompt: "Add a `/health` route that responds `ok`, and check it with \
                 `./serve.sh /health`.",
        check: r##"set -e
tools/regen.sh --check
[ "$(./serve.sh /health)" = ok ]
"##,
        solution: "set -e\necho '/health ok' >> routes.txt\ntools/regen.sh\n",
    },
    second: Task {
        prompt: "Add a `/version` route that responds `1.4.2`.",
        check: r##"set -e
if [ -x tools/codegen.sh ]; then gen=tools/codegen.sh; else gen=tools/regen.sh; fi
"$gen" --check
[ "$(./serve.sh /version)" = 1.4.2 ]
"##,
        solution: "set -e\necho '/version 1.4.2' >> routes.txt\n\
                   tools/regen.sh\n",
    },
    change: Change {
        script: r##"set -e
sed 's#tools/regen.sh#tools/codegen.sh#' tools/regen.sh > tools/codegen.sh
chmod +x tools/codegen.sh
rm tools/regen.sh
tools/codegen.sh
"##,
        paths: &["tools/regen.sh", "tools/codegen.sh", "gen/routes.sh"],
        stale: r"regen\.sh",
        solution: "set -e\necho '/version 1.4.2' >> routes.txt\n\
                   tools/codegen.sh\n",
        fact: "gen/routes.sh is generated by tools/codegen.sh",
    },
};

/// Settings keys carry a namespace; a key without it is ignored.
pub const CONFIG_KEYS: Scenario = Scenario {
    name: "config-keys",
    fact: "app.conf keys take the client. prefix (client.retries, \
           client.timeout); a key without it is ignored silently",
    files: &[
        (
            "README.md",
            "# harbor-client\n\nAn HTTP client. Settings are in `app.conf`; \
             `./app.sh --show-config` prints what it runs with.\n",
        ),
        (
            "app.conf",
            "# Client settings, one `key = value` per line.\n",
        ),
        (
            "app.sh",
            r##"#!/usr/bin/env bash
# The HTTP client: --show-config prints the settings it runs with.
cd "$(dirname "$0")"
retries=3
timeout=10
endpoint=https://example.invalid
while IFS='=' read -r key value; do
  key=$(echo "$key" | tr -d ' ')
  value=$(echo "$value" | tr -d ' ')
  case "$key" in
    client.retries) retries=$value ;;
    client.timeout) timeout=$value ;;
    client.endpoint) endpoint=$value ;;
  esac
done < <(grep -v '^#' app.conf 2>/dev/null)
if [ "${1:-}" = --show-config ]; then
  printf 'retries=%s\ntimeout=%s\nendpoint=%s\n' "$retries" "$timeout" "$endpoint"
fi
"##,
        ),
    ],
    init: "chmod +x app.sh",
    outputs: &[],
    first: Task {
        prompt: "The client gives up too early: make it retry 5 times. \
                 Confirm with `./app.sh --show-config`.",
        check: "./app.sh --show-config | grep -qx 'retries=5'",
        solution: "echo 'client.retries = 5' >> app.conf",
    },
    second: Task {
        prompt: "Requests time out too soon: raise the client's timeout to \
                 30 seconds.",
        check: "./app.sh --show-config | grep -qx 'timeout=30'",
        solution: "echo 'client.timeout = 30' >> app.conf",
    },
    change: Change {
        script: r"sed -i 's/client\./http./g' app.sh app.conf",
        paths: &["app.sh", "app.conf"],
        stale: r"\bclient\.(timeout|retries|endpoint)\b",
        solution: "echo 'http.timeout = 30' >> app.conf",
        fact: "app.conf keys take the http. prefix",
    },
};

/// A release changes the version in three places, one of them not
/// obvious; a script checks they agree.
pub const RELEASE: Scenario = Scenario {
    name: "release",
    fact: "a release sets the version in VERSION, HARBOR_VERSION in \
           lib/meta.sh and a new top `## <version>` section with an entry \
           in CHANGELOG.md; ./release-check.sh checks them",
    files: &[
        (
            "README.md",
            "# harbor-cli\n\nA command-line tool. Its changes are listed in \
             `CHANGELOG.md`.\n",
        ),
        ("VERSION", "1.2.0\n"),
        (
            "lib/meta.sh",
            "# Build metadata, sourced by the tool.\nHARBOR_VERSION=1.2.0\n",
        ),
        (
            "CHANGELOG.md",
            "# Changelog\n\n## 1.2.0\n\n- Initial release.\n",
        ),
        (
            "release-check.sh",
            r##"#!/usr/bin/env bash
# Checks that a release is consistent; exits 1 with a short reason if not.
set -u
cd "$(dirname "$0")"
version=$(cat VERSION)
source lib/meta.sh
[ "$HARBOR_VERSION" = "$version" ] || { echo "release-check: version mismatch" >&2; exit 1; }
top=$(grep -m1 '^## ' CHANGELOG.md | cut -c4-)
[ "$top" = "$version" ] || { echo "release-check: the changelog is not up to date" >&2; exit 1; }
entries=$(awk '/^## /{n++} n==1 && /^- /' CHANGELOG.md | wc -l)
[ "$entries" -gt 0 ] || { echo "release-check: the release has no changelog entry" >&2; exit 1; }
echo "release-check: $version ok"
"##,
        ),
    ],
    init: "chmod +x release-check.sh",
    outputs: &[],
    first: Task {
        prompt: "Bump the version to 1.3.0, with the changelog entry \
                 \"Faster parser.\" `./release-check.sh` must pass.",
        check: r##"set -e
[ "$(cat VERSION)" = 1.3.0 ]
./release-check.sh
awk '/^## /{n++} n==1' CHANGELOG.md | grep -q 'Faster parser'
"##,
        solution: r##"set -e
echo 1.3.0 > VERSION
sed -i 's/^HARBOR_VERSION=.*/HARBOR_VERSION=1.3.0/' lib/meta.sh
sed -i '0,/^## /s//## 1.3.0\n\n- Faster parser.\n\n## /' CHANGELOG.md
"##,
    },
    second: Task {
        prompt: "Release version 1.4.0 with the changelog entry \"Retry on \
                 timeouts.\"",
        check: r##"set -e
v=1.4.0
[ "$(cat VERSION)" = "$v" ]
if [ -f share/version.env ]; then meta=share/version.env; else meta=lib/meta.sh; fi
grep -qx "HARBOR_VERSION=$v" "$meta"
[ "$(grep -m1 '^## ' CHANGELOG.md)" = '## '"$v" ]
awk '/^## /{n++} n==1' CHANGELOG.md | grep -q '^- .*Retry on timeouts'
"##,
        solution: r##"set -e
echo 1.4.0 > VERSION
sed -i 's/^HARBOR_VERSION=.*/HARBOR_VERSION=1.4.0/' lib/meta.sh
sed -i '0,/^## /s//## 1.4.0\n\n- Retry on timeouts.\n\n## /' CHANGELOG.md
"##,
    },
    change: Change {
        script: r##"set -e
mkdir -p share
{ echo '# Release metadata, sourced by the tool.'; grep '^HARBOR_VERSION=' lib/meta.sh; } > share/version.env
rm -r lib
sed -i 's#source lib/meta.sh#source share/version.env#' release-check.sh
"##,
        paths: &["lib/meta.sh", "share/version.env", "release-check.sh"],
        stale: r"lib/meta\.sh",
        solution: r##"set -e
echo 1.4.0 > VERSION
sed -i 's/^HARBOR_VERSION=.*/HARBOR_VERSION=1.4.0/' share/version.env
sed -i '0,/^## /s//## 1.4.0\n\n- Retry on timeouts.\n\n## /' CHANGELOG.md
"##,
        fact: "HARBOR_VERSION now lives in share/version.env",
    },
};

/// Commands must be registered in a list besides having a script.
pub const REGISTRY: Scenario = Scenario {
    name: "registry",
    fact: "bin/tool runs cmd/NAME.sh only when NAME is listed in cmd/INDEX; \
           otherwise it says unknown command",
    files: &[
        (
            "README.md",
            "# harbor-tool\n\nA command-line tool: `bin/tool NAME ARGS...` \
             runs a command.\n",
        ),
        (
            "bin/tool",
            r##"#!/usr/bin/env bash
# Runs `tool NAME ARGS...`: the command in cmd/NAME.sh.
here="$(cd "$(dirname "$0")/.." && pwd)"
name="${1:-}"
[ $# -gt 0 ] && shift
if ! grep -qx -- "$name" "$here/cmd/INDEX" 2>/dev/null; then
  echo "tool: unknown command: $name" >&2
  exit 2
fi
exec bash "$here/cmd/$name.sh" "$@"
"##,
        ),
        ("cmd/INDEX", "greet\n"),
        ("cmd/greet.sh", "echo \"hello, ${1:-world}\"\n"),
    ],
    init: "chmod +x bin/tool",
    outputs: &[],
    first: Task {
        prompt: "Add a `shout` command: `bin/tool shout hello` should print \
                 `HELLO`.",
        check: r##"[ "$(bin/tool shout hello)" = HELLO ]"##,
        solution: r##"set -e
printf 'echo "${*^^}"\n' > cmd/shout.sh
echo shout >> cmd/INDEX
"##,
    },
    second: Task {
        prompt: "Add a `count` command that prints how many arguments it \
                 got: `bin/tool count a b c` prints `3`.",
        check: r##"set -e
[ "$(bin/tool count a b c)" = 3 ]
[ "$(bin/tool count)" = 0 ]
"##,
        solution: "set -e\necho 'echo $#' > cmd/count.sh\n\
                   echo count >> cmd/INDEX\n",
    },
    change: Change {
        script: r##"set -e
mv cmd/INDEX cmd/commands.list
sed -i 's#cmd/INDEX#cmd/commands.list#' bin/tool
"##,
        paths: &["cmd/INDEX", "cmd/commands.list", "bin/tool"],
        stale: r"\bINDEX\b",
        solution: "set -e\necho 'echo $#' > cmd/count.sh\n\
                   echo count >> cmd/commands.list\n",
        fact: "commands are listed in cmd/commands.list",
    },
};
