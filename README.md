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

| Path                       | What                                                                        |
| -------------------------- | --------------------------------------------------------------------------- |
| `template/`                | The template that cargo-generate renders; nothing else in the repo is       |
| `scripts/matrix.tsv`       | The combinations of placeholders that are rendered and checked              |
| `scripts/render.sh`        | Renders combinations into a temporary directory                             |
| `scripts/check.sh`         | Runs each rendered project's own `just check`, outside this repository      |
| `scripts/template-lint.py` | Keeps the template self-contained and its crates within the structure rules |
| `justfile`                 | The maintainer recipes below                                                |

## Maintainer recipes

| Recipe              | What                                                    |
| ------------------- | ------------------------------------------------------- |
| `just lint`         | Template lint, its self-test, and shellcheck            |
| `just render [ids]` | Render the given combinations, or all of them           |
| `just check [ids]`  | Render and check the given combinations, or all of them |
| `just commit`       | The per-commit tier: lint, then render and check `m2`   |

Rendered projects land in `$TMPDIR/rs-starter-template-render/<id>/<name>`; set
`RENDER_DIR` to use another directory and `CG` to use another cargo-generate.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
