# Two stages: build the Rust binary, then ship it on a small runtime image.
# The panel is built separately and served by whatever fronts this — an
# ingress, a CDN — rather than being baked in, so a frontend change does not
# require rebuilding and redeploying the backend.

FROM rust:1.98-slim-bookworm AS build
WORKDIR /src

# Empty for the default build; `aiwatcher-server/laser` to compile the Laser
# backend in. Tilt passes it when the `laser` overlay is selected.
ARG FEATURES=""

RUN apt-get update \
  && apt-get install -y --no-install-recommends pkg-config libssl-dev ca-certificates \
  && rm -rf /var/lib/apt/lists/*

# Manifests first, so a source-only change reuses the dependency layer.
COPY Cargo.toml rust-toolchain.toml ./
COPY crates/aiwatcher-core/Cargo.toml crates/aiwatcher-core/
COPY crates/aiwatcher-bus/Cargo.toml crates/aiwatcher-bus/
COPY crates/aiwatcher-trace/Cargo.toml crates/aiwatcher-trace/
COPY crates/aiwatcher-projector/Cargo.toml crates/aiwatcher-projector/
COPY crates/aiwatcher-api/Cargo.toml crates/aiwatcher-api/
COPY crates/aiwatcher-server/Cargo.toml crates/aiwatcher-server/
RUN for crate in core bus trace projector api server; do \
      mkdir -p "crates/aiwatcher-$crate/src" && \
      echo "" > "crates/aiwatcher-$crate/src/lib.rs"; \
    done \
  && mkdir -p crates/aiwatcher-server/src/bin \
  && echo "fn main() {}" > crates/aiwatcher-server/src/bin/aiwatcher.rs \
  && echo "fn main() {}" > crates/aiwatcher-server/src/bin/aiwatcher-openapi.rs \
  && cargo build --release --bin aiwatcher ${FEATURES:+--features $FEATURES} \
  && rm -rf crates/*/src

COPY crates crates
# Touch the real sources so cargo does not reuse the placeholder artifacts.
RUN find crates -name '*.rs' -exec touch {} + \
  && cargo build --release --bin aiwatcher ${FEATURES:+--features $FEATURES}

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates wget \
  && rm -rf /var/lib/apt/lists/* \
  && useradd --system --uid 10001 --home /var/lib/aiwatcher --create-home aiwatcher

COPY --from=build /src/target/release/aiwatcher /usr/local/bin/aiwatcher

USER aiwatcher
WORKDIR /var/lib/aiwatcher
EXPOSE 8080
ENV AIWATCHER_LISTEN=0.0.0.0:8080 \
    AIWATCHER_DATA_DIR=/var/lib/aiwatcher

ENTRYPOINT ["/usr/local/bin/aiwatcher"]
