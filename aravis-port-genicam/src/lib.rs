#![forbid(unsafe_code)]
//! GenICam XML parsing, node-tree engine, and formula evaluator (Phase 3).

pub mod access;
pub mod dom;
pub mod error;
pub mod formula;
pub mod node;
pub mod tree;

pub use access::{MemoryRegisterAccess, RegisterAccess};
pub use error::{GenIcamError, Result};
pub use node::NodeId;
pub use tree::GenApiTree;
