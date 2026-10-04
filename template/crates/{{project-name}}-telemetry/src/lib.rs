//! Logging: the subscriber with one filter per output, the rolling log file, and the panic
//! hook. Only the binary depends on this crate; every other crate logs through the `tracing`
//! facade.

pub mod error;
pub mod panic;
pub mod rolling;
pub mod settings;
pub mod subscriber;
