//! Logging: the subscriber with its outputs and filters. Only the binary depends on this
//! crate; every other crate logs through the `tracing` facade.

pub mod error;
pub mod settings;
pub mod subscriber;
