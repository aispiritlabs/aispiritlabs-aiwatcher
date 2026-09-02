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
# `Cargo.lock` with them: without it the release build resolves fresh, and the
# binary that ships is built against versions nothing tested.
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
# One line per crate, and it has to be one line per crate: a glob would flatten
# the paths and lose the directory each manifest belongs to. `just check`
# compares this list against `crates/` so a new crate cannot be added without
# it — the list went stale once and the image simply stopped building.
COPY crates/aiwatcher-core/Cargo.toml crates/aiwatcher-core/
COPY crates/aiwatcher-bus/Cargo.toml crates/aiwatcher-bus/
COPY crates/aiwatcher-trace/Cargo.toml crates/aiwatcher-trace/
COPY crates/aiwatcher-jobs/Cargo.toml crates/aiwatcher-jobs/
COPY crates/aiwatcher-prompts/Cargo.toml crates/aiwatcher-prompts/
COPY crates/aiwatcher-annotations/Cargo.toml crates/aiwatcher-annotations/
COPY crates/aiwatcher-datasets/Cargo.toml crates/aiwatcher-datasets/
COPY crates/aiwatcher-training/Cargo.toml crates/aiwatcher-training/
COPY crates/aiwatcher-conversations/Cargo.toml crates/aiwatcher-conversations/
COPY crates/aiwatcher-runner/Cargo.toml crates/aiwatcher-runner/
COPY crates/aiwatcher-pipeline/Cargo.toml crates/aiwatcher-pipeline/
COPY crates/aiwatcher-auth/Cargo.toml crates/aiwatcher-auth/
COPY crates/aiwatcher-projector/Cargo.toml crates/aiwatcher-projector/
COPY crates/aiwatcher-api/Cargo.toml crates/aiwatcher-api/
COPY crates/aiwatcher-server/Cargo.toml crates/aiwatcher-server/
# Derived from the manifests that were actually copied rather than named again.
# Two lists that have to agree is one that can disagree, and the way it fails
# is a build that gets as far as pulling 285 MB of base image first.
RUN for manifest in crates/*/Cargo.toml; do \
      mkdir -p "$(dirname "$manifest")/src" && \
      echo "" > "$(dirname "$manifest")/src/lib.rs"; \
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
