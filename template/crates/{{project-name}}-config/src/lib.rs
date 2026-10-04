//! Layered configuration loading, as a mechanism: the sections and their rules belong to the
//! crates that own them, and the binary joins them into its root configuration.
//!
//! [`load::load`] merges the defaults, a configuration file, aliases such as `RUST_LOG`, the
//! project's environment variables and command-line overrides; records where every key came
//! from; and reports every problem at once as a [`report::Report`]. [`table`] renders the
//! effective values for `check-config` and the startup log.

pub mod load;
pub mod report;
pub mod source;
pub mod table;
