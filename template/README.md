# {{project-name}}

An HTTP service in Rust: a workspace of nine crates in layers, a lifecycle with graceful
shutdown, structured logging, layered configuration and a small example API for todos.
`docs/architecture.md` explains how it is organised and where new code goes;
`AGENTS.md` is the one-page version for people and coding agents making changes.

## Five minutes

1. Run the service; the first build takes a while:

   ```bash
   just run
   ```

   Without just: `cargo run -p {{project-name}} -- run --config config/example.toml`.
   The first log lines give the version and the git commit (`git_sha` is `unknown` until
   the first commit), then `listening http.addr=127.0.0.1:8080` and `phase="running"`.

2. Ask whether it is alive and ready:

   ```bash
   curl -i localhost:8080/livez
   curl -i localhost:8080/readyz
   ```

3. Use the example API. Successes are the resource as JSON; failures are
   `application/problem+json` documents with a `request_id`:

   ```bash
   curl -si -X POST localhost:8080/v1/todos -H 'content-type: application/json' -d '{"title":"try the template"}'
   curl -si localhost:8080/v1/todos/01999999-9999-7999-8999-999999999999
   curl -si -X POST localhost:8080/v1/todos/<id from the first answer>/complete -H 'content-type: application/json' -d '{"version": 99}'
   curl -si -X POST localhost:8080/v1/todos -H 'content-type: application/json' -d '{"title": ""}'
   ```

   The answers are 201 with a `Location` header, then 404, 409 and 422.

4. Stop it with Ctrl-C, or `kill -TERM <pid>` from another terminal. The log shows the
   phases `stopping` and `stopped` (`draining` first after SIGTERM), and its last line is `stopped exit_code=0`;
   after Ctrl-C, just adds `error: interrupted by SIGINT`, since the terminal sends the
   signal to just as well. With SIGTERM, `/readyz` answers 503 at once while requests are
   still served for `lifecycle.drain_delay` (5 seconds), so a load balancer can take the
   instance out first.

5. Run every check CI runs:

   ```bash
   just check
   ```

6. Commit (cargo-generate has already created the git repository), and the startup line
   shows the commit:

   ```bash
   git add -A && git commit -m init && just run
   ```

## Tools

The Rust toolchain comes from `rust-toolchain.toml` (stable, with rustfmt and clippy). The
recipes need:

```bash
cargo install --locked just cargo-nextest cargo-deny typos-cli cargo-machete
```

`just msrv` needs Rust 1.88 as well (`rustup toolchain install 1.88`), and
`just console-check` builds with tokio-console support.
{%- if with_ci %} `just workflows` needs
[actionlint](https://github.com/rhysd/actionlint).{% endif %}
{%- if with_docker %} `just docker-build` needs Docker.{% endif %}

The service runs on Linux, macOS and Windows. On Windows the recipes need bash, as Git for
Windows provides; the console events stand for the signals (see `docs/architecture.md`),
and the tests that send signals run on Unix only.
{%- if with_docker %} The container image is Linux only.{% endif %}

## Commands

| Command | What it does |
| --- | --- |
| `just run` | Run the service with `config/example.toml` |
| `just check-config` | Check the configuration and print every key's value and where it came from |
| `just check` | Everything below from `fmt-check` to `{% if with_ci %}workflows{% else %}deny{% endif %}`, in order |
| `just fmt-check` | Formatting |
| `just lint` | Clippy on every target, with default and with all features |
| `just test` | Tests and doctests |
| `just doc` | API documentation without warnings |
| `just deny` | Licenses, security advisories and sources of the dependencies |
| `just typos` | Spelling |
| `just machete` | Unused dependencies |
{%- if with_ci %}
| `just workflows` | Lint the GitHub Actions workflows |
{%- endif %}
| `just console` | Run with [tokio-console](https://github.com/tokio-rs/console) support |
| `just console-check` | Check the console feature |
| `just msrv` | Check that Rust 1.88 builds the project |
| `just package` | Build `dist/{{project-name}}-<version>-<target>.tar.gz` and its `.sha256` |
{%- if with_docker %}
| `just docker-build` | Build the image `{{project-name}}:dev` |
{%- endif %}

Build with `--features mimalloc` to replace the system allocator with
[mimalloc](https://github.com/microsoft/mimalloc); measure whether it helps your load.

The binary has three subcommands: `run`, `check-config` and `probe <URL>`, which sends one
GET and exits 0 for a 2xx answer (the container health check uses it). Configuration
comes from the defaults, the file given with `--config` or
`{{project-name | shouty_snake_case}}_CONFIG`, `RUST_LOG` (for the log filter only),
`{{project-name | shouty_snake_case}}_<SECTION>__<KEY>` environment variables such as
`{{project-name | shouty_snake_case}}_SERVER__HTTP_ADDR=0.0.0.0:8080`, and the flags
`--http-addr`, `--log-filter` and `--log-format`, in that order of priority.

## Cargo.lock

Commit `Cargo.lock`: this is an application, and its builds should use the dependency
versions that were tested. The first build creates the file.
{%- if with_ci %} CI stops with a clear error when it is missing and builds with `--locked`
{%- if with_docker %}, and so does the Docker build{% endif %}.
{%- elsif with_docker %} The Docker build stops with a clear error when it is missing and
builds with `--locked`.
{%- endif %}
Locally, the recipes add `--locked` only when the environment variable
`CARGO_LOCKED=--locked` is set, so a new project builds before the first lock file exists.
{%- if with_docker %}

## Container image

```bash
just docker-build
docker run --rm -p 8080:8080 {{project-name}}:dev
```

The image runs on distroless as a non-root user and listens on `0.0.0.0:8080`; its health
check calls the binary's `probe` on `/livez`, so a container that drains or waits for a
dependency stays healthy (traffic decisions belong to `/readyz`). The license texts are in
`/usr/share/doc/{{project-name}}/`.
{%- endif %}
{%- if with_ci %}

## Continuous integration

`.github/workflows/ci.yml` runs `just check`, the check with Rust 1.88 and the console check on
every push and pull request. Pushing a tag such as `v0.1.0` builds the release archive and
publishes it with its SHA-256 as a GitHub Release
{%- if with_docker %}, and builds, smoke-tests and pushes the image to
`ghcr.io/<owner>/<repository>`{% endif %}.
{%- endif %}

## License
{% if license != "None" %}
{{license}}; see `LICENSE`. The project includes code from pingora-error under the Apache
License 2.0; see `THIRD_PARTY_NOTICES.md`.
{%- else %}
No license is declared. The project includes code from pingora-error under the Apache
License 2.0; see `THIRD_PARTY_NOTICES.md`, which every distribution must include.
{%- endif %}
