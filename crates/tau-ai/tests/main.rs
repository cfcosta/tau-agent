//! Every integration test of tau-ai, in one binary: linking one
//! rather than one per file is most of what building the tests
//! costs (docs/reference/testing.md, "One binary per crate").

mod chatgpt;
mod chatgpt_properties;
mod chatgpt_store;
mod chatgpt_transport;
mod client;
mod continuation;
mod cost;
mod event;
mod files;
mod lane;
mod message;
mod model;
mod partial_json;
mod plan_models;
mod pool;
mod prompt_cache;
mod responses_input;
mod responses_request;
mod responses_stream;
mod retry;
mod transport;
mod ws_connection;
