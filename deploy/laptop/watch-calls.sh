#!/usr/bin/env bash
# Streams oFono call signals (CallAdded / CallRemoved / PropertyChanged) so you
# can see RING, the caller number (LineIdentification), and state transitions
# while testing. Ctrl-C to stop.
#
# Uses dbus-monitor with signal match rules: signals are broadcast, so no
# root/BecomeMonitor privilege is needed (busctl monitor would require it).
#
# To answer the ringing call from another terminal:
#   busctl call org.ofono <call path> org.ofono.VoiceCall Answer
# To hang up:
#   busctl call org.ofono <call path> org.ofono.VoiceCall Hangup
# (call path looks like /hfp/org/bluez/hci0/dev_XX_XX_XX_XX_XX_XX/voicecall01)
exec /usr/bin/dbus-monitor --system \
  "type='signal',sender='org.ofono',interface='org.ofono.VoiceCallManager'" \
  "type='signal',sender='org.ofono',interface='org.ofono.VoiceCall'"
