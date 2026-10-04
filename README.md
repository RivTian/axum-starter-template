# rs-starter-template

A [cargo-generate](https://github.com/cargo-generate/cargo-generate) template for a
multi-crate Rust service.

```bash
cargo generate RivTian/rs-starter-template
```

> Status: under construction (sixth edition). The design is
> [docs/prompts/01-design-rust-template-v6.md](docs/prompts/01-design-rust-template-v6.md);
> progress is in [docs/log.md](docs/log.md).

## Repository layout

| Path                              | What                                                                                     |
| --------------------------------- | ---------------------------------------------------------------------------------------- |
| `template/`                       | The template that cargo-generate renders; nothing else in the repo is                    |
| `scripts/matrix.tsv`              | The combinations of placeholders that are rendered and checked                           |
| `scripts/render.sh`               | Renders combinations into a temporary directory                                          |
| `scripts/check.sh`                | Runs each rendered project's checks, outside this repository                             |
| `scripts/template-lint.py`        | Keeps the template self-contained and its crates within the structure rules              |
| `scripts/vendor-pingora-error.py` | Vendors pingora-error and proves the vendored files are upstream plus the listed patches |
| `scripts/names.sh`, `names.exp`   | The project-name rules on every input path                                               |
| `scripts/version-info.sh`         | The binary's version information in six git scenarios                                    |

## Maintainer recipes

| Recipe              | What                                                                                       |
| ------------------- | ------------------------------------------------------------------------------------------ |
| `just lint`         | Template lint and its self-test, the vendored files, shellcheck, actionlint                |
| `just render [ids]` | Render the given combinations, or all of them                                              |
| `just check [ids]`  | Render and run each project's own `just check`                                             |
| `just full [ids]`   | Render and run the full check: smoke runs, MSRV, console, package, lock-file errors, image |
| `just commit`       | The per-commit tier: lint, then render and check `m2`                                      |
| `just names`        | The project-name rules                                                                     |
| `just version-info` | The version information scenarios                                                          |
| `just ci`           | Everything the template CI runs, in the same order                                         |

Rendered projects land in `$TMPDIR/rs-starter-template-render/<id>/<name>`; set
`RENDER_DIR` to use another directory and `CG` to use another cargo-generate. The full check
builds container images only under the tag `rs-starter-template-check-<name>` and removes
them afterwards.

The template requires and is tested with one cargo-generate version, set in
`template/cargo-generate.toml` and in `.github/workflows/template-ci.yml`; raise both together.
`just ci` runs what CI runs, with the `cargo-generate` on `PATH`.

To update the vendored pingora-error, change the pinned commit and hashes in
`scripts/vendor-pingora-error.py`, run it, and review the diff.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
