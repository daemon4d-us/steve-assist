#!/usr/bin/env bash
# Verifies the oFono + PipeWire HFP setup. Safe to run any time.
set -uo pipefail
ok()   { printf '  [ok]   %s\n' "$*"; }
warn() { printf '  [!!]   %s\n' "$*"; }

echo "-- ofono service"
systemctl is-active --quiet ofono && ok "ofono.service active" || warn "ofono.service not running"

echo "-- D-Bus access as $USER"
if busctl call org.ofono / org.ofono.Manager GetModems >/dev/null 2>&1; then
  ok "can call org.ofono.Manager.GetModems"
else
  warn "org.ofono not reachable as $USER (policy not installed, dbus not reloaded, or ofono down)"
fi

echo "-- oFono modems (one per connected HFP phone)"
modems=$(busctl call org.ofono / org.ofono.Manager GetModems 2>/dev/null | grep -oE '"/hfp/[^"]+"' || true)
if [ -n "$modems" ]; then
  echo "$modems" | sed 's/^/       /'
  # oFono does not implement org.freedesktop.DBus.Properties; use its own GetProperties.
  for m in $(echo "$modems" | tr -d '"'); do
    props=$(busctl call org.ofono "$m" org.ofono.Modem GetProperties 2>/dev/null)
    online=$(echo "$props" | grep -oE '"Online" v b (true|false)' | awk '{print $NF}')
    ifaces=$(echo "$props" | grep -oE 'org\.ofono\.[A-Za-z]+' | sort -u | tr '\n' ' ')
    echo "       Online=${online:-?}  Interfaces: ${ifaces}"
  done
  echo "-- oFono audio cards (appear once PipeWire registered its audio agent)"
  cards_out=$(busctl call org.ofono / org.ofono.HandsfreeAudioManager GetCards 2>/dev/null)
  echo "$cards_out" | grep -q '"/' && ok "$(echo "$cards_out" | grep -oE '"/[^"]+"' | tr '\n' ' ')" \
    || warn "no HandsfreeAudioCard yet (PipeWire oFono backend not running, or SLC not up)"
else
  warn "no HFP modem yet (phone not paired/connected, or HFP disabled on the phone)"
fi

echo "-- PipeWire HFP backend"
if grep -q 'bluez5.hfphsp-backend"\] = "ofono"' ~/.config/wireplumber/bluetooth.lua.d/51-steve-bluez.lua 2>/dev/null; then
  ok "51-steve-bluez.lua installed (backend = ofono)"
else
  warn "~/.config/wireplumber/bluetooth.lua.d/51-steve-bluez.lua missing"
fi

echo "-- Bluetooth cards and profiles"
cards=$(pactl list cards short 2>/dev/null | awk '$2 ~ /^bluez_card/ {print $2}')
if [ -n "$cards" ]; then
  for c in $cards; do
    prof=$(pactl list cards 2>/dev/null | awk -v c="$c" '$0 ~ "Name: "c {f=1} f && /Active Profile/ {print $3; exit}')
    ok "$c active profile: ${prof:-?}"
    [ "$prof" = "headset-head-unit" ] || [ "${prof#headset-head-unit}" != "$prof" ] \
      || echo "       switch with: pactl set-card-profile $c headset-head-unit"
  done
else
  warn "no bluez card (pair and connect the phone first)"
fi

echo "-- Bluetooth audio nodes"
pw-dump 2>/dev/null | grep -oE '"node.name": "bluez_(input|output)[^"]*"' | sort -u | sed 's/^/       /' || true
