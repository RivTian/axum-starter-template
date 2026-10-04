# Working on {{project-name}}

For people and coding agents changing this project. `docs/architecture.md` is the contract;
this page is what to know before a change and what to do before calling it done.

## Before a change is done

- `just check` passes: formatting, Clippy with warnings as errors, the tests, the docs,
  spelling, unused and denied dependencies. Never commit with a failing check.
- New behaviour comes with a test that fails without it.
- Do not silence a lint with `#[allow]`. Where a rule truly does not fit, use
  `#[expect(lint, reason = "...")]` on the smallest item.
- One change per commit, with a message that says what changed and why.
- A change written by an agent is reviewed by someone else (a person or another agent)
  before it is merged.

## Where code goes

| You add                 | Put it in                                                                  |
| ----------------------- | -------------------------------------------------------------------------- |
| an entity or a rule     | the feature's module in `crates/{{project-name}}-domain`                   |
| a use case              | the feature's `usecases.rs`                                                |
| a port                  | the feature's `ports.rs`; the implementation in `crates/{{project-name}}-infra` |
| an error                | one constant in the feature's or the crate's `error.rs`                    |
| an endpoint             | the feature's module in `crates/{{project-name}}-api`, one line in `router.rs` |
| a background task       | a `Service` in infra, one line in `crates/{{project-name}}/src/wiring.rs`  |
| a configuration key     | the owning crate's `settings.rs`, and `config/example.toml`                |

## Rules the tests enforce

`crates/{{project-name}}/tests/architecture.rs` fails the build when a change breaks one:

- Dependencies between crates point downwards only, along the table in
  `docs/architecture.md`; test-utils is only a dev-dependency.
- util, domain and config pull in no async runtime or networking crate; runtime and
  telemetry pull in no HTTP stack.
- No crate depends directly on anyhow, eyre, color-eyre or failure.
- Dependency versions live in `[workspace.dependencies]`; every crate uses the workspace
  lints.
- `Custom`, `CustomCode`, `ReusedOnly`, `new_str` and `new_code` of the error type appear
  only in its vendored code: an error kind is an `ErrorKind`.

Changing a rule means changing `docs/architecture.md` and the test in the same commit.

## Conventions

- **Errors**: one error type, `svc_util::error::Error`. An error kind is a constant with a
  class (`ErrorKind::new("TodoNotFound", Class::NotFound)`); add `.titled(..)` when callers
  see it. Pass errors up with `?`; log an error once, where it is handled, with
  `log_error!`. Never log and return the same error.
- **The vendored error code** in `crates/{{project-name}}-util/src/error/pingora*` is
  pingora-error with one added variant (see `THIRD_PARTY_NOTICES.md`). Leave it as it is.
- **Configuration**: fields read text from variables and flags, so give each one a
  deserializer from `svc_util::de` and a default instead of an `Option`. Keep
  `config/example.toml` equal to the defaults; a test checks it.
- **Logging**: through `tracing`, with OpenTelemetry semantic-convention field names such as
  `http.response.status_code`. `println!` and `eprintln!` are denied.
- **Tests** return `Result` and use `?`; `unwrap`, `expect` and `panic!` are denied.
- **Files** stay under 500 lines; a feature is `feature.rs` with a `feature/` directory, and
  `lib.rs` only declares modules.

## Commands

| Command             | What                                             |
| ------------------- | ------------------------------------------------ |
| `just run`          | Run the service with `config/example.toml`       |
| `just check`        | Every check CI runs                              |
| `just test`         | Tests and doctests                               |
| `just check-config` | Print every configuration key and its source     |
| `just fmt`          | Format the code                                  |
