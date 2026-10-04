# The service image: built with the latest stable Rust, run on distroless (no shell) as a
# non-root user. The health check uses the binary's own probe. Build with `just docker-build`
# so that the image knows the current commit.

FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
ARG GIT_SHA=unknown
ENV VERGEN_GIT_SHA=${GIT_SHA}
RUN test -f Cargo.lock || { echo "Cargo.lock is missing: run cargo generate-lockfile and commit it" >&2; exit 1; }
RUN cargo build --release --locked --bin {{project-name}}

FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=build /src/target/release/{{project-name}} /usr/local/bin/{{project-name}}
{% if license != "None" -%}
COPY --from=build /src/THIRD_PARTY_NOTICES.md /src/LICENSE /usr/share/doc/{{project-name}}/
{% else -%}
COPY --from=build /src/THIRD_PARTY_NOTICES.md /usr/share/doc/{{project-name}}/
{% endif -%}
ENV {{project-name | shouty_snake_case}}_SERVER__HTTP_ADDR=0.0.0.0:8080
EXPOSE 8080
USER nonroot:nonroot
HEALTHCHECK --interval=10s --timeout=3s --start-period=10s --retries=3 \
  CMD ["/usr/local/bin/{{project-name}}", "probe", "http://127.0.0.1:8080/readyz"]
ENTRYPOINT ["/usr/local/bin/{{project-name}}"]
CMD ["run"]
