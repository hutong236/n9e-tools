FROM rust:1-bookworm AS build
WORKDIR /app
COPY Cargo.toml ./
COPY src ./src
COPY static ./static
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/* && useradd --system --uid 10001 --no-create-home n9e
COPY --from=build /app/target/release/n9e-tools /usr/local/bin/n9e-tools
USER 10001:10001
ENV BIND=0.0.0.0:8088
EXPOSE 8088
ENTRYPOINT ["/usr/local/bin/n9e-tools"]
