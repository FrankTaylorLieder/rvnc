#!/bin/sh
set -e
GEOMETRY="${GEOMETRY:-1280x800}"

# Stale locks from a previous run stop the X servers starting after a restart.
rm -f /tmp/.X*-lock /tmp/.X11-unix/X*

# TigerVNC on :1 (port 5901), no auth, resizable via xrandr.
Xvnc :1 -geometry "$GEOMETRY" -depth 24 -SecurityTypes None \
     -AlwaysShared -localhost=0 -rfbport 5901 -AcceptSetDesktopSize=1 &
# x11vnc on :2 (port 5902), sharing an Xvfb display.
Xvfb :2 -screen 0 "${GEOMETRY}x24" &
sleep 1
x11vnc -display :2 -nopw -forever -shared -rfbport 5902 -quiet &

# Plain background, and stop fluxbox nagging about wallpaper setters.
mkdir -p ~/.fluxbox && echo "background: none" > ~/.fluxbox/overlay
for d in 1 2; do
  DISPLAY=:$d xsetroot -solid '#3a6ea5'
  DISPLAY=:$d fluxbox >/dev/null 2>&1 &
  DISPLAY=:$d xterm -geometry 100x30+40+40 -fa 'DejaVu Sans Mono' -fs 12 &
done

wait
