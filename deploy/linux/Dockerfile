FROM --platform=linux/amd64 rust:1.99.1-slim-bookworm AS build
RUN apt-get update -qq && apt-get install -y --no-install-recommends git lsof procps pkg-config \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /work
COPY . .
RUN cargo build --release --locked -j 4 -p mpac-daemon -p mpac-mobile-link \
    --bin mpac-daemon --bin mpac-mobile-host

FROM scratch
COPY --from=build /work/target/release/mpac-daemon /mpac-daemon
COPY --from=build /work/target/release/mpac-mobile-host /mpac-mobile-host
