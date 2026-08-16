FROM alpine:3.20
RUN apk add --no-cache ca-certificates libgcc libstdc++

# Non-root runtime identity. This image runs an agent that executes tools and
# shell commands: as uid 0, any harness escape or hostile tool call would act
# as root inside the container, and as root on the host through a bind-mounted
# workspace. Fixed uid/gid 10001 so host-side `chown` on bind mounts is stable.
RUN addgroup -g 10001 ryuzi \
 && adduser -D -u 10001 -G ryuzi -h /home/ryuzi ryuzi

COPY ryuzi /usr/local/bin/ryuzi

# The daemon resolves its state dir from `dirs::data_dir()` and its config dir
# from `dirs::config_dir()` (crates/core/src/paths.rs), both of which need HOME
# on Linux. Docker does NOT populate HOME from /etc/passwd for a USER, and
# without it `state_dir()` degrades to the relative path "./ryuzi".
ENV HOME=/home/ryuzi

# Both dirs must EXIST and be owned by uid 10001 BEFORE the VOLUME lines:
# Docker seeds an anonymous volume from the image directory and inherits its
# ownership, so a missing or root-owned directory here produces a volume the
# non-root daemon cannot write to.
RUN mkdir -p /home/ryuzi/.local/share/ryuzi /home/ryuzi/.config/ryuzi \
 && chown -R 10001:10001 /home/ryuzi

# State: ryuzi.sqlite, control.token, tls_cert.pem/tls_key.pem, daemon.json.
# Config: agent YAML profiles + per-agent knowledge, installed plugin bundles.
VOLUME ["/home/ryuzi/.local/share/ryuzi", "/home/ryuzi/.config/ryuzi"]

# Control API port (`control_port`, default 4483 — see DEFAULT_CONTROL_PORT in
# crates/runner/src/daemon_cmd.rs). Documentation only: it publishes nothing by
# itself, and the daemon stays on loopback until `listen_addr` is widened.
EXPOSE 4483

USER 10001:10001
ENTRYPOINT ["ryuzi"]
CMD ["start"]
