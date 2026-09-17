#!/usr/bin/env bash
# Milestone-1 end-to-end audio test. Waits for an incoming call on the oFono
# HFP modem, prints the caller number, answers, records N seconds of far-end
# audio from the PipeWire Bluetooth source, plays a tone into the call, hangs up.
#   deploy/laptop/answer-test.sh [seconds=5] [out.wav] [play.wav]
# If play.wav is given it is played into the call (after the tone) so the
# far end hears it; with Steve on the other end, its transcript then shows
# what the laptop said.
set -uo pipefail
SECS="${1:-5}"; OUT="${2:-/tmp/steve-bt-test.wav}"; PLAY="${3:-}"
MODEM=$(busctl call org.ofono / org.ofono.Manager GetModems 2>/dev/null | grep -oE '"/hfp/[^"]+"' | head -1 | tr -d '"')
[ -n "$MODEM" ] || { echo "no HFP modem"; exit 1; }
echo "modem: $MODEM  (waiting for an incoming call, Ctrl-C to abort)"
CALL=""
while [ -z "$CALL" ]; do
  CALL=$(busctl call org.ofono "$MODEM" org.ofono.VoiceCallManager GetCalls 2>/dev/null | grep -oE '"'"$MODEM"'/voicecall[0-9]+"' | head -1 | tr -d '"')
  sleep 0.5
done
PROPS=$(busctl call org.ofono "$CALL" org.ofono.VoiceCall GetProperties 2>/dev/null)
echo "call: $CALL"
echo "$PROPS" | grep -oE '"(LineIdentification|Name|State)" s "[^"]*"' | sed 's/^/  /'
STATE=$(echo "$PROPS" | grep -oE '"State" s "[^"]*"' | cut -d'"' -f4)
if [ "$STATE" = "incoming" ]; then
  for a in 1 2 3 4 5; do
    echo "answering (attempt $a)..."
    if busctl call org.ofono "$CALL" org.ofono.VoiceCall Answer 2>&1; then break; fi
    sleep 1
  done
  echo "state now: $(busctl call org.ofono "$CALL" org.ofono.VoiceCall GetProperties 2>/dev/null | grep -oE '"State" s "[^"]*"' | cut -d'"' -f4)"
fi
# PipeWire exposes the phone's SCO audio as two *stream* nodes (not a plain
# source/sink): bluez_input.<addr>.N carries far-end voice (Stream/Output/Audio,
# auto-linked to the default sink) and bluez_output.<addr>.N takes audio into
# the call (Stream/Input/Audio). They exist only while the SCO link is up.
# pw-dump can emit several concatenated JSON documents, so decode them in a loop.
list_bt_nodes() {
  pw-dump 2>/dev/null | python3 -c '
import json,sys
d=json.JSONDecoder(); s=sys.stdin.read(); i=0; objs=[]
while i < len(s):
    while i < len(s) and s[i] in " \n\r\t": i+=1
    if i >= len(s): break
    o,i=d.raw_decode(s,i); objs.extend(o if isinstance(o,list) else [o])
for o in objs:
    if o.get("type")!="PipeWire:Interface:Node": continue
    p=o.get("info",{}).get("props",{}); n=p.get("node.name","")
    if n.startswith("bluez_"): print(n, p.get("media.class",""), p.get("factory.name",""))'
}
echo "waiting for Bluetooth audio nodes (up to 40s)..."
for i in $(seq 1 80); do
  NODES=$(list_bt_nodes)
  SRC=$(echo "$NODES" | awk '$1 ~ /^bluez_input/ {print $1; exit}')
  SNK=$(echo "$NODES" | awk '$1 ~ /^bluez_output/ {print $1; exit}')
  [ -n "$SRC" ] && [ -n "$SNK" ] && break
  [ $((i % 10)) -eq 0 ] && echo "  t=$((i/2))s nodes: ${NODES:-none}"
  sleep 0.5
done
echo "source: ${SRC:-none}   sink: ${SNK:-none}"
[ -n "$NODES" ] && echo "$NODES" | sed 's/^/  node: /'
if [ -n "$SRC" ]; then
  echo "recording ${SECS}s of far-end audio to $OUT (say something on the calling phone)"
  timeout "$((SECS+2))" pw-record --target "$SRC" --rate 8000 --channels 1 --format s16 "$OUT" &
  RP=$!
fi
if [ -n "$SNK" ]; then
  echo "playing a 1 kHz tone into the call for 2s (should be audible on the calling phone)"
  python3 - <<'PY'
import math,struct,wave
w=wave.open('/tmp/steve-tone.wav','wb'); w.setnchannels(1); w.setsampwidth(2); w.setframerate(8000)
w.writeframes(b''.join(struct.pack('<h', int(12000*math.sin(2*math.pi*1000*i/8000))) for i in range(16000))); w.close()
PY
  timeout 5 pw-play --target "$SNK" /tmp/steve-tone.wav
  if [ -n "$PLAY" ] && [ -f "$PLAY" ]; then
    echo "playing $PLAY into the call"
    timeout 30 pw-play --target "$SNK" "$PLAY"
  fi
fi
[ -n "${RP:-}" ] && wait $RP 2>/dev/null
sleep 1
echo "hanging up"; busctl call org.ofono "$CALL" org.ofono.VoiceCall Hangup 2>/dev/null
[ -f "$OUT" ] && echo "recorded: $(stat -c %s "$OUT") bytes -> $OUT (play with: pw-play $OUT)"
