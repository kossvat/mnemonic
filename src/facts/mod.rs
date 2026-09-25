//! Facts: values that belong to a slot (project, subject, predicate,
//! qualifier) and change over time. The chain keeps every value with the
//! time it held; the current one is derived, never stored.

pub mod declare;
pub mod digest;
pub mod keys;
pub mod legacy;
pub mod mcp_tools;
pub mod rekey;
pub mod render;
pub mod rule;
pub mod schema;
pub mod store;
pub mod view;
