#!/usr/bin/env bash
# One-time laptop setup for the Steve Bluetooth bridge (milestone 1 of
# docs/plans/bluetooth-bridge.md). Installs oFono, grants the desktop user
# D-Bus access to it, switches PipeWire's HFP backend to oFono, and restarts
# the audio session. Idempotent; re-run freely. Asks for sudo where needed.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
USER_NAME="${SUDO_USER:-$USER}"

echo "==> Installing ofono"
sudo apt-get install -y ofono

echo "==> D-Bus policy for user ${USER_NAME}"
sed "s/user=\"dsidorenko\"/user=\"${USER_NAME}\"/" "$HERE/steve-ofono.dbus.conf" \
  | sudo tee /etc/dbus-1/system.d/steve-ofono.conf >/dev/null
# dbus-daemon reloads policy on SIGHUP; systemd's reload does that.
sudo systemctl reload dbus

echo "==> WirePlumber config"
mkdir -p ~/.config/wireplumber/bluetooth.lua.d
cp "$HERE/wireplumber/51-steve-bluez.lua" ~/.config/wireplumber/bluetooth.lua.d/

# Order matters: PipeWire's native backend owns the HFP HF profile until it is
# restarted with the oFono backend. If ofono starts first, BlueZ refuses its
# RegisterProfile ("UUID already registered") and nobody serves HFP.
echo "==> Restarting the user audio session (PipeWire + WirePlumber)"
systemctl --user restart wireplumber pipewire pipewire-pulse
sleep 2

echo "==> Enabling and (re)starting ofono.service"
sudo systemctl enable ofono >/dev/null
sudo systemctl restart ofono
sleep 1
if bluetoothctl show | grep -q "UUID: Handsfree "; then
  echo "  [ok]   adapter advertises Handsfree (oFono owns HFP HF)"
else
  echo "  [!!]   adapter does not advertise Handsfree; check: journalctl -u ofono -n 20"
fi

echo "==> Checks"
"$HERE/check-hfp.sh"

cat <<'MSG'

Next, at the machine:
  1. Pair the Pixel:  bluetoothctl  ->  scan on, pair <MAC>, trust <MAC>, connect <MAC>
  2. On the Pixel, keep "Phone calls" enabled for the laptop in the device's Bluetooth settings.
  3. Run deploy/laptop/check-hfp.sh again: it should list an oFono modem for the phone.
  4. Run deploy/laptop/watch-calls.sh, then call the Pixel from another phone.
MSG
