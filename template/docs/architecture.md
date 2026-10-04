# {{project-name}} architecture

How the workspace is organised, which rules keep it that way, and where new code goes.

## Crates and layers

Nine crates in five layers. Dependencies point downwards only, and crates on the same layer
do not depend on each other. Library names are `svc_<role>`, so source code and log targets
do not change with the project name.

| Layer | Crate (library)                                   | Holds                                                     |
| ----- | ------------------------------------------------- | --------------------------------------------------------- |
| 4     | `{{project-name}}` (`svc_app`)                    | the command line, the root configuration, the wiring      |
| 3     | `-api` (`svc_api`), `-infra` (`svc_infra`)        | HTTP; adapters for the domain's ports                     |
| 2     | `-runtime` (`svc_runtime`), `-telemetry` (`svc_telemetry`) | services and their supervisor; logging           |
| 1     | `-domain` (`svc_domain`), `-config` (`svc_config`) | entities, use cases and ports; configuration loading     |
| 0     | `-util` (`svc_util`)                              | the error type, logging macros, settings helpers          |
| tests | `-test-utils` (`svc_test_utils`)                  | test doubles, used only as a dev-dependency               |

`crates/{{project-name}}/tests/architecture.rs` checks these rules on every test run. Each
rule has a test that breaks it on purpose.

| Rule              | What it requires                                                                 | Why                                              |
| ----------------- | -------------------------------------------------------------------------------- | ------------------------------------------------ |
| layers            | members depend only on the members listed for them                              | changes flow downwards; same-layer crates stay replaceable |
| dev-only          | test-utils is only ever a dev-dependency                                         | test doubles never reach the binary              |
| closure           | util, domain and config pull in no runtime or networking crate; runtime and telemetry pull in no HTTP stack | the domain and configuration can be tested anywhere; the runtime serves any transport |
| one-error-model   | no member depends directly on anyhow, eyre, color-eyre or failure                | one error type in the project's own code         |
| workspace-versions | every dependency is declared in `[workspace.dependencies]`                      | one place to upgrade                             |
| workspace-lints   | every member uses `[lints] workspace = true`                                     | one set of lints                                 |
| upstream-names    | `Custom`, `CustomCode`, `ReusedOnly`, `new_str` and `new_code` of the error type appear only in its vendored file | an error kind is an `ErrorKind`, and no retry decision is left open |

Inside a crate: one concept per module, features grouped as `todo.rs` with `todo/`, no
`mod.rs` in `src`, at most one directory below a top-level module, and no file over 500
lines apart from the vendored pingora-error file.
`lib.rs` only declares modules; a top-level module may re-export items of its private
submodules, so every public path has two segments, such as `svc_util::error::Error`. Crates
that other crates use a lot (util, domain, runtime) also have a `prelude`.

## Where does a new feature go

| You add                 | Put it in                                                                 | Example                          |
| ----------------------- | ------------------------------------------------------------------------- | -------------------------------- |
| an entity or a rule     | the feature's module in the domain                                        | `todo/model.rs`: `Title`         |
| a use case              | the feature's `usecases.rs`, over ports                                   | `TodoUseCases::complete`         |
| a port                  | the feature's `ports.rs`, as an object-safe trait                         | `TodoRepository`                 |
| an error                | one constant in the feature's or the crate's `error.rs`                   | `TODO_NOT_FOUND`                 |
| an adapter              | infra, implementing the port; checked against the port's contract in test-utils | `memory/todo.rs`           |
| an endpoint             | the feature's module in api, and one line in `router.rs`                  | `todo.rs`                        |
| a background task       | a type in infra implementing `svc_runtime::service::Service`, and one line in `wiring.rs` | `event_log.rs`   |
| a configuration key     | the owning crate's `settings.rs`, and `config/example.toml`               | `server.request_timeout`         |
| a readiness check       | a type implementing `svc_runtime::health::HealthCheck`, registered in `wiring.rs` | a database connection     |

## Errors

The whole workspace uses one error type, `svc_util::error::Error`, from pingora-error (see
`THIRD_PARTY_NOTICES.md`): a type, a source (upstream, downstream or internal), a retry
decision, a context and a cause. An error kind of this project carries its class, and a
title when callers see it as a client error:

```rust
pub const TODO_NOT_FOUND: ErrorType =
    ErrorType::Kind(&ErrorKind::new("TodoNotFound", Class::NotFound).titled("Todo not found"));
```

Adapters mark failures of a dependency with `into_up()` and set `retry` when a retry may
help. An error is logged once, where it is handled: by the HTTP layer when it answers, or by
the supervisor when a service fails. The fields are `error.type`, `error.class`,
`error.source`, `error.retry`, `error.context`, `error.chain` and `error.cause`, the text of
the first error from outside the project, such as the operating system's. Lists of problems, such as
an invalid configuration, are data and not errors.

## HTTP contract

A success is the resource itself as JSON: 200, 201 with `Location`, 202, or 204 without a
body. A failure is an `application/problem+json` document, rendered by one middleware
whatever produced it, the framework's own answers included:

| Member       | Value                                                                                       |
| ------------ | ------------------------------------------------------------------------------------------- |
| `type`       | `urn:{{project-name}}:problem:<kind>` for a 4xx of an error kind, such as `todo-not-found`; `about:blank` otherwise |
| `title`      | the kind's title for those; the status reason otherwise                                     |
| `status`     | the status code                                                                             |
| `detail`     | the error's context, for 4xx other than 401 and 403                                         |
| `instance`   | the request path                                                                            |
| `request_id` | the same value as the `x-request-id` response header                                        |

The class gives the status code; a client error caused by a dependency is this service's
failure:

| Class             | Status | Caused by a dependency |
| ----------------- | ------ | ---------------------- |
| `InvalidInput`    | 422    | 500                    |
| `Unauthenticated` | 401    | 500                    |
| `Forbidden`       | 403    | 500                    |
| `NotFound`        | 404    | 500                    |
| `Conflict`        | 409    | 500                    |
| `TooManyRequests` | 429    | 503                    |
| `Unavailable`     | 503    | 503                    |
| `Timeout`         | 504    | 504                    |
| `Internal`        | 500    | 500                    |

The status gives the rest: 429, 503 and 504 are logged as warnings, other 5xx as errors,
4xx at debug; `Retry-After: 1` comes with 429 and 503 when the error says a retry may help.
A request that takes longer than `server.request_timeout` gets 503. A request id sent in
`x-request-id` is kept when it is 1 to 128 of `A-Z a-z 0-9 . _ -`; otherwise the service
makes a `UUIDv7`. Requests that the HTTP library rejects before routing, such as a
malformed request line, get its own plain answer, not a problem document.

## Lifecycle

`svc_runtime::supervisor::Supervisor` starts every service at once. Everything that happens
(a service ready, returning, failing or panicking, a signal, a timer) goes through one
channel and is handled in order by a state machine, so the same events always end the same
way. A frontline service (the HTTP server) takes traffic; a background service (the event
log) works behind it.

| Phase      | `/readyz` | Enters when                                                         |
| ---------- | --------- | ------------------------------------------------------------------- |
| `starting` | 503       | the process starts                                                  |
| `running`  | 200       | every service called `ready()`                                      |
| `draining` | 503       | SIGTERM while running; requests are still served for `drain_delay`  |
| `stopping` | 503       | after the drain delay, SIGINT, a failure, or startup failing; frontline services stop first, then background ones |
| `stopped`  | -         | every service has stopped, or the deadline passed                   |

A service that fails, panics or returns before it called `ready()` fails the startup (69),
and so does `startup_timeout`. Once a service is ready, the same is a fault (70), whatever
the other services are doing; only a background service may return `Ok`. A failure during a signal-started shutdown turns it into a fault. A second SIGTERM or
SIGINT stops at once; SIGHUP is logged and ignored. A client that never finishes sending its
request headers keeps its connection open until the deadline, and the exit code is then 75.

| Exit code | Meaning                                                                    |
| --------- | -------------------------------------------------------------------------- |
| 0         | stopped after a signal; `check-config` passed; help or version             |
| 1         | `probe` only: no 2xx answer                                                |
| 64        | the command line is wrong                                                  |
| 69        | the service could not start, for example because the port is in use      |
| 70        | a fault while running or while stopping                                    |
| 71        | the runtime or the signal handlers could not be set up                     |
| 73        | logging could not start, for example because the log directory cannot be created |
| 75        | services were still running at the shutdown deadline                       |
| 78        | the configuration is invalid                                               |
| 101       | the main thread panicked, which is a bug                                   |
| 128 + n   | a second signal cut the shutdown short (130, 143)                          |

## Configuration

Each crate owns its section (`ServerSettings` in api, `LifecycleSettings` and
`EventSettings` in runtime, `LogSettings` in telemetry) with its defaults and its value
checks; `svc_config` only loads, and the binary joins the sections in `settings.rs`.
`config/example.toml` lists every key with its default.

Sources, lowest priority first: the defaults; the file given with `--config` or
`{{project-name | shouty_snake_case}}_CONFIG`; `RUST_LOG` for `log.filter` (empty counts as
unset); variables `{{project-name | shouty_snake_case}}_<SECTION>__<KEY>`, such as
`{{project-name | shouty_snake_case}}_LOG__FILE__ENABLED=true`; and the flags `--http-addr`,
`--log-filter` and `--log-format`. Prefixed variables without `__`, such as the ones
Kubernetes adds for a Service of the same name, are ignored. Unknown keys are errors. Every
problem is reported at once, one line each, with the key and where its value came from, and
the process exits with 78. `check-config` runs the same checks and prints every key with its
value and source; `run` logs the keys that differ from their defaults. Every value is shown
as it is, so keep secrets out of the configuration.

A new key needs a deserializer from `svc_util::de` (or one of its own, as the log filters
have): variables and flags give every value as text. Give it a default instead of making it
an `Option`, since the serialized defaults are the list of keys.

## Logging

Logs go to stdout: text on a terminal, JSON otherwise. With `log.file.enabled = true` they
also go to `<log.file.dir>/{{project-name}}.log`, rolled at UTC midnight into compressed
archives, of which the newest `log.file.max_archives` are kept. Each output has its own
filter; nothing filters them all. Every request runs in a span named `request` with
`request_id`, created at error level so that warnings and errors carry it whatever the
filter; `request finished` (target `svc_api::access`) closes each request, also one the
client gave up on. A panic is logged with target `panic`, or written to stderr when that
target is filtered out. `just console` runs with tokio-console support.

| Purpose                                  | Filter                       |
| ---------------------------------------- | ---------------------------- |
| the default                              | `info`                       |
| without the access log                   | `info,svc_api::access=warn`  |
| the reasons of 4xx answers               | `info,svc_api=debug`         |
| only the lifecycle, warnings and errors  | `warn,svc_runtime=info`      |

## Testing

| Kind          | Where                                       | What                                                    |
| ------------- | ------------------------------------------- | ------------------------------------------------------- |
| unit          | `#[cfg(test)]` modules                      | one module's logic; the supervisor's table cell by cell |
| crate         | each crate's `tests/`                       | a crate through its public items, with test-utils       |
| HTTP contract | `crates/{{project-name}}-api/tests/`        | every response shape, through the router               |
| supervisor    | `crates/{{project-name}}-runtime/tests/`    | scripted services on paused time, each scenario 50 times |
| process       | `crates/{{project-name}}/tests/process.rs`  | the real binary: exit codes, signals, `check-config`, `probe` |
| architecture  | `crates/{{project-name}}/tests/architecture.rs` | the rules above                                     |

Tests return `Result` and use `?`: the lints forbid `unwrap` and `expect` everywhere. `just
test` runs them with cargo-nextest, which gives every test its own process, and then the
doctests.
