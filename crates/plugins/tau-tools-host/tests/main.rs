//! Every integration test of tau-tools-host, in one binary: linking one
//! rather than one per file is most of what building the tests
//! costs (docs/reference/testing.md, "One binary per crate").

mod common;

mod act;
mod agent;
mod artifact_ranges;
mod edit;
mod errno;
mod exec;
mod exec_launch;
mod find;
mod grep;
mod http;
mod image;
mod lock;
mod ls;
mod path;
mod read;
mod truncate;
mod write;
