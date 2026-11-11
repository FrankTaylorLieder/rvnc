# Development VNC servers for rvnc. No authentication: for local use only.
FROM debian:bookworm-slim

RUN apt-get update \
 && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
      tigervnc-standalone-server tigervnc-tools x11vnc xvfb \
      fluxbox xterm x11-utils x11-xserver-utils xdotool \
      fonts-dejavu-core procps \
 && rm -rf /var/lib/apt/lists/*

COPY entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

# 5901: TigerVNC Xvnc (display :1). 5902: x11vnc sharing an Xvfb (display :2).
EXPOSE 5901 5902
ENTRYPOINT ["/entrypoint.sh"]
