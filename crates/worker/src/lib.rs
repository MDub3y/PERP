//! Exposes the worker's task modules as a library, purely so integration
//! tests (crates/tests) can call `deposit_indexer`/`sweeper`/`processor`
//! functions directly against a real Postgres/Solana test-validator
//! instance without shelling out to the compiled binary. `main.rs` uses
//! these same modules through this crate rather than declaring its own
//! private `mod` tree.

pub mod consumer;
pub mod deposit_indexer;
pub mod processor;
pub mod reconciler;
pub mod relay;
pub mod sweeper;
pub mod wallet;
