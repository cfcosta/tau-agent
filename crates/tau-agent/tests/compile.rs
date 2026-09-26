//! The public API rejects misuse at compile time (`trybuild`).
//!
//! The expected compiler output in `tests/ui/*.stderr` depends on the
//! toolchain the flake pins; regenerate it with `TRYBUILD=overwrite`
//! after a toolchain update, and review the diff.

#[test]
fn misuse_does_not_compile() {
    let cases = trybuild::TestCases::new();
    cases.compile_fail("tests/ui/*.rs");
}

/// What must compile: agents and runs cross threads.
#[test]
fn agents_and_runs_are_send() {
    fn send_sync<T: Send + Sync>() {}
    fn send<T: Send>() {}
    send_sync::<tau_agent::agent::Agent>();
    send::<tau_agent::agent::Run>();
    send_sync::<tau_agent::tool::ToolOutput>();
}
