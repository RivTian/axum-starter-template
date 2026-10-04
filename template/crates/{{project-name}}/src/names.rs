//! Names derived from the project name when the project was generated.

/// The name of the service, used in logs, problem types and the version output.
#[rustfmt::skip]
pub(crate) const SERVICE_NAME: &str = "{{project-name}}";

/// The prefix of the environment variables that override configuration keys.
#[rustfmt::skip]
pub(crate) const ENV_PREFIX: &str = "{{project-name | shouty_snake_case}}";
