# syntax=docker/dockerfile:1
FROM rust:1.90-bookworm AS builder
WORKDIR /src
COPY . .
ARG SOURCE_DATE_EPOCH=1
ENV SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH}
RUN cargo build --release --locked -p meld-cli -p meld-lsp -p meld-installer

FROM debian:bookworm-slim
LABEL org.opencontainers.image.title="meld-lang" \
      org.opencontainers.image.source="https://github.com/enis1enis2/meld" \
      org.opencontainers.image.description="Meld language-composition compiler, CLI and language server"
WORKDIR /work
COPY --from=builder /src/target/release/meld /usr/local/bin/meld
COPY --from=builder /src/target/release/meld-lsp /usr/local/bin/meld-lsp
COPY --from=builder /src/target/release/meld-pack /usr/local/bin/meld-pack
COPY --from=builder /src/packs /usr/share/meld-lang/packs
COPY --from=builder /src/examples /usr/share/meld-lang/examples
COPY --from=builder /src/README.md /usr/share/doc/meld-lang/README.md
COPY --from=builder /src/CHANGELOG.md /usr/share/doc/meld-lang/CHANGELOG.md
COPY --from=builder /src/RELEASE.md /usr/share/doc/meld-lang/RELEASE.md
COPY --from=builder /src/SBOM.md /usr/share/doc/meld-lang/SBOM.md
COPY --from=builder /src/SECURITY.md /usr/share/doc/meld-lang/SECURITY.md
COPY --from=builder /src/THIRD-PARTY-NOTICES.md /usr/share/doc/meld-lang/THIRD-PARTY-NOTICES.md
COPY --from=builder /src/LICENSE-APACHE /usr/share/doc/meld-lang/LICENSE-APACHE
COPY --from=builder /src/LICENSE-MIT /usr/share/doc/meld-lang/LICENSE-MIT
COPY --from=builder /src/NOTICE /usr/share/doc/meld-lang/NOTICE
ENV MELD_PACKS=/usr/share/meld-lang/packs
ENTRYPOINT ["/usr/local/bin/meld"]
CMD ["doctor"]
