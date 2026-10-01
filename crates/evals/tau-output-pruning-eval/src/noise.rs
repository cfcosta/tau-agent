//! Synthetic noise in the style of real tools: build progress, package
//! downloads, test progress. Nothing here is ever needed, and no noise
//! line says error, fail or warning.

use crate::rng::Rng;

const CRATES: &[&str] = &[
    "serde",
    "tokio",
    "hyper",
    "rustls",
    "regex",
    "syn",
    "quote",
    "libc",
    "bytes",
    "mio",
    "tracing",
    "futures-util",
    "http",
    "url",
    "ring",
    "parking_lot",
    "smallvec",
    "memchr",
    "once_cell",
    "rand",
    "clap",
    "anyhow",
    "thiserror",
    "indexmap",
    "hashbrown",
    "itoa",
    "ryu",
    "proc-macro2",
    "unicode-ident",
    "cfg-if",
    "log",
    "percent-encoding",
];

const PACKAGES: &[&str] = &[
    "react",
    "lodash",
    "typescript",
    "esbuild",
    "vite",
    "eslint",
    "chalk",
    "semver",
    "debug",
    "ms",
    "glob",
    "minimatch",
    "rimraf",
    "yargs",
    "commander",
    "postcss",
    "autoprefixer",
    "tslib",
    "zod",
    "undici",
    "@babel/core",
    "@babel/parser",
    "@types/node",
    "rollup",
    "picocolors",
];

const PYTHON: &[&str] = &[
    "numpy",
    "pandas",
    "requests",
    "urllib3",
    "certifi",
    "idna",
    "pydantic",
    "fastapi",
    "starlette",
    "uvicorn",
    "sqlalchemy",
    "alembic",
    "jinja2",
    "markupsafe",
    "click",
    "attrs",
    "packaging",
    "pyyaml",
    "httpx",
    "anyio",
    "sniffio",
    "h11",
    "typing-extensions",
    "tzdata",
    "six",
];

const WORDS: &[&str] = &[
    "ledger",
    "invoice",
    "account",
    "session",
    "cache",
    "queue",
    "router",
    "billing",
    "search",
    "index",
    "report",
    "export",
    "import",
    "tenant",
    "profile",
    "payment",
    "refund",
    "coupon",
    "catalog",
    "inventory",
    "shipping",
    "audit",
    "token",
    "webhook",
    "schedule",
    "metrics",
];

fn version(rng: &mut Rng) -> String {
    format!(
        "{}.{}.{}",
        rng.range(0, 5),
        rng.range(0, 40),
        rng.range(0, 20)
    )
}

/// `name_name` from [`WORDS`].
pub fn ident(rng: &mut Rng) -> String {
    format!("{}_{}", rng.pick(WORDS), rng.pick(WORDS))
}

/// `cargo build` progress.
pub fn cargo(rng: &mut Rng, n: usize) -> String {
    let krate = rng.pick(CRATES);
    let v = version(rng);
    match rng.range(0, 10) {
        0..=4 => format!("   Compiling {krate} v{v}"),
        5 | 6 => {
            format!("    Checking {krate}-{n} v{v} (/work/crates/{krate}-{n})")
        }
        7 | 8 => format!("  Downloaded {krate} v{v}"),
        _ => format!(
            "    Building [{:=<width$}>{:width2$}] {n}/9812: {krate}(build), {}",
            "",
            "",
            rng.pick(CRATES),
            width = rng.range(1, 40),
            width2 = rng.range(1, 20),
        ),
    }
}

/// `npm install` fetches.
pub fn npm(rng: &mut Rng, _n: usize) -> String {
    let package = rng.pick(PACKAGES);
    match rng.range(0, 6) {
        0..=3 => format!(
            "npm http fetch GET 200 https://registry.npmjs.org/{package} {}ms (cache revalidated)",
            rng.range(3, 900)
        ),
        4 => format!(
            "npm timing reify:loadTrees:{package} Completed in {}ms",
            rng.range(1, 400)
        ),
        _ => format!(
            "npm sill tarball no local data for {package}@{}. Extracting by manifest.",
            version(rng)
        ),
    }
}

/// `pytest -v` progress, or a `cargo test` pass: always passing.
pub fn tests(rng: &mut Rng, n: usize, cargo_style: bool) -> String {
    if cargo_style {
        format!("test {}::{}::case_{n} ... ok", rng.pick(WORDS), ident(rng))
    } else {
        format!(
            "tests/{}/test_{}.py::test_{}_{n} PASSED{:>w$}[{:>3}%]",
            rng.pick(WORDS),
            rng.pick(WORDS),
            ident(rng),
            "",
            rng.range(0, 100),
            w = rng.range(1, 12),
        )
    }
}

/// `pip install` downloads.
pub fn pip(rng: &mut Rng, _n: usize) -> String {
    let package = rng.pick(PYTHON);
    let v = version(rng);
    match rng.range(0, 5) {
        0 => format!("Collecting {package}=={v}"),
        1 => format!(
            "  Downloading {package}-{v}-py3-none-any.whl.metadata ({} kB)",
            rng.range(1, 90)
        ),
        2 => format!(
            "  Downloading {package}-{v}-cp312-cp312-manylinux_2_17_x86_64.whl ({}.{} MB)",
            rng.range(0, 40),
            rng.range(0, 10)
        ),
        3 => format!(
            "     {} {}.{}/{}.{} MB {}.{} MB/s eta 0:00:0{}",
            "━".repeat(40),
            rng.range(0, 20),
            rng.range(0, 10),
            rng.range(20, 40),
            rng.range(0, 10),
            rng.range(1, 90),
            rng.range(0, 10),
            rng.range(0, 10),
        ),
        _ => format!(
            "Requirement already satisfied: {package}>={v} in ./.venv/lib/python3.12/site-packages (from -r requirements.txt (line {}))",
            rng.range(1, 80)
        ),
    }
}

/// A bundler's per-module progress.
pub fn bundler(rng: &mut Rng, n: usize) -> String {
    match rng.range(0, 3) {
        0 => format!(
            "transforming ({n}) src/{}/{}.tsx",
            rng.pick(WORDS),
            ident(rng)
        ),
        1 => format!(
            "  dist/assets/{}-{}.js   {}.{} kB │ gzip: {}.{} kB",
            rng.pick(WORDS),
            rng.hex(8),
            rng.range(1, 900),
            rng.range(10, 99),
            rng.range(1, 200),
            rng.range(10, 99)
        ),
        _ => format!("rendering chunks ({n})... {}", rng.pick(PACKAGES)),
    }
}

/// An end-to-end runner's steps, all passing.
pub fn e2e(rng: &mut Rng, n: usize, total: usize) -> String {
    format!(
        "[e2e {n:>5}/{total}] ok   {}::{} ({}ms, worker {})",
        rng.pick(WORDS),
        ident(rng),
        rng.range(2, 2400),
        rng.range(0, 16)
    )
}

/// Container layers being pulled and uploaded, for a release.
pub fn upload(rng: &mut Rng, _n: usize) -> String {
    let layer = rng.hex(12);
    match rng.range(0, 3) {
        0 => format!(
            "{layer}: Pushing [{:=<w$}>{:w2$}] {}.{}MB/{}MB",
            "",
            "",
            rng.range(1, 90),
            rng.range(0, 10),
            rng.range(90, 400),
            w = rng.range(1, 40),
            w2 = rng.range(1, 12)
        ),
        1 => format!("{layer}: Layer already exists"),
        _ => format!("{layer}: Pushed"),
    }
}

/// A sentence of runbook prose.
pub fn prose(rng: &mut Rng, n: usize) -> String {
    let subjects = [
        "The release captain",
        "The on-call engineer",
        "Whoever cuts the release",
        "The build farm",
        "The staging cluster",
        "The deploy bot",
    ];
    let verbs = [
        "checks",
        "rotates",
        "reviews",
        "archives",
        "verifies",
        "announces",
        "tags",
        "rebuilds",
        "signs",
        "mirrors",
    ];
    format!(
        "{n}. {} {} the {} {} before the {} window, and records it in the {} channel.",
        rng.pick(&subjects),
        rng.pick(&verbs),
        rng.pick(WORDS),
        rng.pick(&[
            "dashboards",
            "manifests",
            "snapshots",
            "credentials",
            "runbooks"
        ]),
        rng.pick(WORDS),
        rng.pick(WORDS),
    )
}
