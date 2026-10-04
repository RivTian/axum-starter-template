//! Path of the binary under test.

/// The service binary that Cargo built for this test run.
#[rustfmt::skip]
pub(crate) const BIN: &str = env!("CARGO_BIN_EXE_{{project-name}}");
