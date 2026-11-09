//! Every integration test of tau-codemode-host, in one binary: linking one
//! rather than one per file is most of what building the tests
//! costs (docs/reference/testing.md, "One binary per crate").

mod common;

mod http;
mod infer_tool;
mod inference;
mod inference_budget;
mod inference_trace;
mod jev;
mod json;
mod map;
mod mapping;
mod module_loading;
mod module_promotion;
mod module_tests;
mod module_tools;
mod options;
mod outline;
mod plugin;
mod sandbox;
mod search;
mod signature;
mod ui;
