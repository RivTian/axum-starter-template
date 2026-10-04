//! The version and the git commit the binary was built from (`build.rs`).

use std::sync::OnceLock;

/// The version of the service.
pub(crate) const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The full SHA of the commit the binary was built from, or `unknown` when the build had
/// no git information.
pub(crate) fn git_sha() -> &'static str {
    match env!("VERGEN_GIT_SHA") {
        "VERGEN_IDEMPOTENT_OUTPUT" => "unknown",
        sha => sha,
    }
}

/// `0.1.0 (1a2b3c4)`: the version and the first 7 characters of the SHA, as `--version`
/// shows them after the name.
pub(crate) fn version() -> &'static str {
    static VERSION_LINE: OnceLock<String> = OnceLock::new();
    VERSION_LINE.get_or_init(|| {
        let sha = git_sha();
        format!("{VERSION} ({})", sha.get(..7).unwrap_or(sha))
    })
}
