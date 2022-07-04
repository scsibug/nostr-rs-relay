FROM docker.io/library/rust:1.62.0@sha256:8220e4fbb22a07b78e6472cdf8f5fb8913a04974c26b130177b73a8a64334541 as builder

RUN USER=root cargo new --bin nostr-rs-relay
WORKDIR ./nostr-rs-relay
COPY ./Cargo.toml ./Cargo.toml
COPY ./Cargo.lock ./Cargo.lock
RUN cargo build --release
RUN rm src/*.rs

COPY ./src ./src

RUN rm ./target/release/deps/nostr*relay*
RUN cargo build --release

FROM docker.io/library/debian:bullseye-20220622-slim@sha256:89e9d812b34f393bddc3ff289f0036a3d9efc7e2f7289ec902c6427b69f39649
ARG APP=/usr/src/app
ARG APP_DATA=/usr/src/app/db
RUN apt-get update \
    && apt-get install -y ca-certificates tzdata sqlite3 libc6 \
    && rm -rf /var/lib/apt/lists/*

EXPOSE 8080

ENV TZ=Etc/UTC \
    APP_USER=appuser

RUN groupadd $APP_USER \
    && useradd -g $APP_USER $APP_USER \
    && mkdir -p ${APP} \
    && mkdir -p ${APP_DATA}

COPY --from=builder /nostr-rs-relay/target/release/nostr-rs-relay ${APP}/nostr-rs-relay

RUN chown -R $APP_USER:$APP_USER ${APP}

USER $APP_USER
WORKDIR ${APP}

ENV RUST_LOG=info
ENV APP_DATA=${APP_DATA}

CMD ./nostr-rs-relay --db ${APP_DATA}
