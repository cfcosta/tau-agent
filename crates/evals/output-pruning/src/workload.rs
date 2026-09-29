//! The workloads: a conversation that ends in one long command output,
//! with the lines the task needs (needles) among lines it does not
//! (noise), generated from a seed.
//!
//! After the evaluations of `tamaratran/jev-pruner` (`evals/*/prompt.md`),
//! with larger outputs, since tau prunes only past 10,000 estimated
//! tokens, and three kinds of its own: a needle `bash`'s tail loses,
//! needles far apart, and a needle only an earlier tool result makes
//! relevant.

use serde::Serialize;
use serde_json::{Value, json};

use crate::{noise, rng::Rng};

/// What a workload tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    /// One error line deep in a noisy build.
    NeedleError,
    /// A value the user asked to remember, mid-output among lines just
    /// like it.
    NeedleDetail,
    /// The install's totals line, near the end.
    SummaryLine,
    /// A large JSON document; the task needs one record of it.
    StructuredJson,
    /// Nothing the task needs: pruning should cut most of it.
    AllNoise,
    /// A failure far above the 2,000 lines `bash` keeps.
    SpilledMiddle,
    /// Three failures far apart.
    MultiNeedle,
    /// A line only an earlier tool result, one too large to share a
    /// state with the output, says is needed.
    EarlierRequirement,
}

impl Kind {
    pub const ALL: [Kind; 8] = [
        Kind::NeedleError,
        Kind::NeedleDetail,
        Kind::SummaryLine,
        Kind::StructuredJson,
        Kind::AllNoise,
        Kind::SpilledMiddle,
        Kind::MultiNeedle,
        Kind::EarlierRequirement,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Kind::NeedleError => "needle-error",
            Kind::NeedleDetail => "needle-detail",
            Kind::SummaryLine => "summary-line",
            Kind::StructuredJson => "structured-json",
            Kind::AllNoise => "all-noise",
            Kind::SpilledMiddle => "spilled-middle",
            Kind::MultiNeedle => "multi-needle",
            Kind::EarlierRequirement => "earlier-requirement",
        }
    }

    pub fn parse(name: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|kind| kind.name() == name)
    }

    pub fn describe(self) -> &'static str {
        match self {
            Kind::NeedleError => "one error line deep in a noisy cargo build",
            Kind::NeedleDetail => {
                "a bundle hash the user asked to remember, among hundreds like it"
            }
            Kind::SummaryLine => {
                "npm's totals line, with post-install noise after it"
            }
            Kind::StructuredJson => {
                "a service registry as JSON; the task needs one service's record"
            }
            Kind::AllNoise => "a pip install the task needs nothing from",
            Kind::SpilledMiddle => {
                "a failed step far above the 2,000-line tail bash keeps"
            }
            Kind::MultiNeedle => {
                "three failing tests far apart in a cargo test run"
            }
            Kind::EarlierRequirement => {
                "a checksum only a large runbook read earlier asks for"
            }
        }
    }
}

/// A tool call made before the command, and its result.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Earlier {
    pub tool: String,
    pub args: Value,
    pub result: String,
}

/// A line the task needs, and where it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Needle {
    /// The line's index in the output, counting from 0.
    pub line: usize,
    pub text: String,
}

/// One workload.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Workload {
    pub kind: Kind,
    pub seed: u64,
    /// The user's prompt.
    pub prompt: String,
    /// Tool calls before the command, in order.
    pub earlier: Vec<Earlier>,
    /// The `bash` command.
    pub command: String,
    /// Its whole output.
    pub output: String,
    pub needles: Vec<Needle>,
}

/// Lines of noise from `line`, with needles at the chosen indices.
struct Builder {
    lines: Vec<String>,
    needles: Vec<Needle>,
}

impl Builder {
    fn new() -> Self {
        Self {
            lines: Vec::new(),
            needles: Vec::new(),
        }
    }

    fn noise(&mut self, line: String) {
        self.lines.push(line);
    }

    fn needle(&mut self, text: String) {
        self.needles.push(Needle {
            line: self.lines.len(),
            text: text.clone(),
        });
        self.lines.push(text);
    }

    fn output(self) -> (String, Vec<Needle>) {
        (self.lines.join("\n"), self.needles)
    }
}

/// The workload of `kind` for `seed`: the same for the same pair.
pub fn generate(kind: Kind, seed: u64) -> Workload {
    let mut rng = Rng::new(
        seed.wrapping_mul(0x2545_f491_4f6c_dd1d) ^ (kind as u64 + 1) << 56,
    );
    let rng = &mut rng;
    let mut builder = Builder::new();
    let mut earlier = Vec::new();
    let (prompt, command) = match kind {
        Kind::NeedleError => {
            let total = rng.range(2_600, 3_400);
            let at = rng.range(total * 3 / 10, total * 7 / 10);
            let krate = format!(
                "{}-{}",
                noise::ident(rng).replace('_', "-"),
                rng.hex(4)
            );
            for n in 0..total {
                if n == at {
                    builder.needle(format!(
                        "error[E0308]: mismatched types in `{krate}` at src/{}.rs:{}:{}: expected `u64`, found `Option<u64>`",
                        noise::ident(rng),
                        rng.range(10, 900),
                        rng.range(1, 80)
                    ));
                } else {
                    builder.noise(noise::cargo(rng, n));
                }
            }
            (
                "Run `cargo build --workspace` and tell me in one sentence which crate failed to build and why.".to_owned(),
                "cargo build --workspace 2>&1".to_owned(),
            )
        }
        Kind::NeedleDetail => {
            let bundles = rng.range(1_600, 2_200);
            let wanted = rng.range(bundles / 4, bundles * 3 / 4);
            for n in 0..bundles {
                let line = format!(
                    "bundle Q{n} hash={} size={}.{}kB",
                    rng.hex(16),
                    rng.range(10, 900),
                    rng.range(10, 99)
                );
                if n == wanted {
                    builder.needle(line);
                } else {
                    builder.noise(line);
                }
                if rng.one_in(3) {
                    builder.noise(noise::bundler(rng, n));
                }
            }
            builder.noise(format!(
                "✓ built {bundles} bundles in {}.{}s",
                rng.range(4, 90),
                rng.range(0, 10)
            ));
            (
                format!(
                    "Build every report bundle with `npm run bundle:all`. Remember the bundle hash for Q{wanted}: I'll need it for the release notes after."
                ),
                "npm run bundle:all".to_owned(),
            )
        }
        Kind::SummaryLine => {
            let fetches = rng.range(2_600, 3_300);
            for n in 0..fetches {
                builder.noise(noise::npm(rng, n));
            }
            let added = rng.range(100, 900);
            builder.needle(format!(
                "added {added} packages, and audited {} packages in {}s",
                added + rng.range(1, 400),
                rng.range(8, 120)
            ));
            let after = rng.range(40, 90);
            for n in 0..after {
                builder.noise(format!(
                    "> postinstall-{n}@{}.{}.{} node scripts/postinstall.js --quiet ({}ms)",
                    rng.range(0, 9),
                    rng.range(0, 9),
                    rng.range(0, 9),
                    rng.range(1, 300)
                ));
            }
            (
                "Install the dependencies with `npm install` and tell me how many packages were added and how long the install took.".to_owned(),
                "npm install --loglevel http".to_owned(),
            )
        }
        Kind::StructuredJson => {
            let services = rng.range(500, 600);
            let wanted = rng.range(services / 5, services * 4 / 5);
            let teams = ["infra", "platform", "data", "edge"];
            let owner = format!(
                "{}-{}-team",
                noise::ident(rng).replace('_', "-"),
                rng.hex(4)
            );
            builder.noise("{".into());
            builder.noise("  \"services\": [".into());
            for id in 0..services {
                builder.noise("    {".into());
                let fields = [
                    format!("      \"id\": {id},"),
                    format!("      \"name\": \"svc-{id}\","),
                    format!("      \"port\": {},", 8000 + id),
                    format!(
                        "      \"owner\": \"{}\",",
                        if id == wanted {
                            owner.as_str()
                        } else {
                            rng.pick(&teams)
                        }
                    ),
                ];
                for field in fields {
                    if id == wanted {
                        builder.needle(field);
                    } else {
                        builder.noise(field);
                    }
                }
                builder.noise(format!(
                    "      \"region\": \"{}\"",
                    rng.pick(&["us-east-1", "eu-west-1", "ap-south-1"])
                ));
                builder.noise(
                    if id + 1 == services {
                        "    }"
                    } else {
                        "    },"
                    }
                    .into(),
                );
            }
            builder.noise("  ]".into());
            builder.noise("}".into());
            (
                format!(
                    "Export the service registry with `svcctl registry export --json` and tell me which team owns svc-{wanted} and which port it listens on."
                ),
                "svcctl registry export --json".to_owned(),
            )
        }
        Kind::AllNoise => {
            let total = rng.range(2_400, 3_000);
            for n in 0..total {
                builder.noise(noise::pip(rng, n));
            }
            builder.noise(
                "Installing collected packages: numpy, pandas, requests".into(),
            );
            builder
                .noise("Successfully installed numpy pandas requests".into());
            (
                "Install the Python dependencies with `pip install -r requirements.txt`, then start on the migration script.".to_owned(),
                "pip install -r requirements.txt".to_owned(),
            )
        }
        Kind::SpilledMiddle => {
            let total = rng.range(3_200, 3_800);
            let at = rng.range(total / 6, total - 2_400);
            let step = noise::ident(rng);
            for n in 0..total {
                if n == at {
                    builder.needle(format!(
                        "[e2e {n:>5}/{total}] FAIL {step}_v{}: checksum mismatch (expected {}, got {})",
                        rng.range(2, 9),
                        rng.hex(12),
                        rng.hex(12)
                    ));
                } else {
                    builder.noise(noise::e2e(rng, n, total));
                }
            }
            builder.noise(format!(
                "e2e: {} of {total} steps passed, 1 did not; {}s",
                total - 1,
                rng.range(300, 900)
            ));
            (
                "Run the end-to-end suite with `make e2e` and tell me which step failed and why.".to_owned(),
                "make e2e".to_owned(),
            )
        }
        Kind::MultiNeedle => {
            let total = rng.range(3_000, 3_800);
            let spots: Vec<usize> = [15, 50, 85]
                .iter()
                .map(|percent| {
                    let centre = total * percent / 100;
                    rng.range(centre - total / 20, centre + total / 20)
                })
                .collect();
            builder.noise(format!("running {total} tests"));
            for n in 0..total {
                if spots.contains(&n) {
                    builder.needle(format!(
                        "test {}::{}::case_{n} ... FAILED",
                        noise::ident(rng),
                        noise::ident(rng)
                    ));
                } else {
                    builder.noise(noise::tests(rng, n, true));
                }
            }
            builder.noise(String::new());
            builder.noise(format!(
                "test result: FAILED. {} passed; 3 failed; 0 ignored; 0 measured; 0 filtered out; finished in {}.{}s",
                total - 3,
                rng.range(10, 90),
                rng.range(0, 99)
            ));
            (
                "Run the whole test suite with `cargo test --workspace` and list every failing test by name.".to_owned(),
                "cargo test --workspace 2>&1".to_owned(),
            )
        }
        Kind::EarlierRequirement => {
            let artifacts = rng.range(350, 450);
            let wanted = rng.range(artifacts / 5, artifacts * 4 / 5);
            let wanted_name = format!(
                "{}-{}",
                noise::ident(rng).replace('_', "-"),
                rng.hex(6)
            );
            // The runbook: prose past one state, the requirement in its
            // middle.
            let paragraphs = rng.range(500, 560);
            let requirement = rng.range(paragraphs * 2 / 5, paragraphs * 3 / 5);
            let mut runbook = vec!["# Release runbook".to_owned()];
            for n in 0..paragraphs {
                if n == requirement {
                    runbook.push(format!(
                        "{n}. IMPORTANT: when `./release.sh` runs, copy the sha256 line for `{wanted_name}.tar.gz` into the release notes verbatim. Nothing else from its output matters."
                    ));
                } else {
                    runbook.push(noise::prose(rng, n));
                }
            }
            earlier.push(Earlier {
                tool: "read".into(),
                args: json!({"path": "docs/RELEASE.md"}),
                result: runbook.join("\n"),
            });
            for n in 0..artifacts {
                let name = if n == wanted {
                    wanted_name.clone()
                } else {
                    format!(
                        "{}-{}",
                        noise::ident(rng).replace('_', "-"),
                        rng.hex(6)
                    )
                };
                let line =
                    format!("sha256 {}  dist/{name}.tar.gz", rng.hex(64));
                if n == wanted {
                    builder.needle(line);
                } else {
                    builder.noise(line);
                }
                if rng.one_in(2) {
                    builder.noise(noise::upload(rng, n));
                }
            }
            builder.noise(format!(
                "release: {artifacts} artifacts signed and pushed"
            ));
            (
                "Do today's release: read docs/RELEASE.md, then run `./release.sh` and do what the runbook says.".to_owned(),
                "./release.sh".to_owned(),
            )
        }
    };
    let (output, needles) = builder.output();
    Workload {
        kind,
        seed,
        prompt,
        earlier,
        command,
        output,
        needles,
    }
}
