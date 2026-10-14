# Builds one Vigil service. Select it with --build-arg BIN=vigil-indexer (or risk-engine, liquidator).
FROM rust:1-bookworm AS build
WORKDIR /src
COPY services/ services/
COPY migrations/ migrations/
ARG BIN
RUN cd services && cargo build --release -p ${BIN}

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
ARG BIN
COPY --from=build /src/services/target/release/${BIN} /usr/local/bin/service
ENTRYPOINT ["/usr/local/bin/service"]
