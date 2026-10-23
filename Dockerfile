# PieceOff room server image.
#
#   docker build -t pieceoff-server .
#   docker run -d --name pieceoff -p 28878:28878 -v pieceoff-data:/data pieceoff-server
#
# See docs/DEPLOYMENT.md for the settings (PIECEOFF_*) and for running behind
# a TLS reverse proxy.

FROM rust:1.98-slim-bookworm AS build
WORKDIR /src
# Only the server and the core crate it uses are compiled; the desktop app
# (and its audio/GUI system libraries) never enter the build.
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo build --release --locked -p pieceoff-server \
    && mkdir /data-skeleton

FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=build /src/target/release/pieceoff-server /pieceoff-server
# A named volume mounted here inherits this ownership (the nonroot user).
COPY --from=build --chown=65532:65532 /data-skeleton /data
ENV PIECEOFF_BIND=0.0.0.0:28878 \
    PIECEOFF_DATA_DIR=/data
EXPOSE 28878
VOLUME /data
# Rooms are saved when the server stops; `docker stop` sends SIGTERM and the
# server writes everything out before exiting.
STOPSIGNAL SIGTERM
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD ["/pieceoff-server", "--healthcheck"]
USER nonroot
ENTRYPOINT ["/pieceoff-server"]
