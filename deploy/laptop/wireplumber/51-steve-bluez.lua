-- WirePlumber 0.4 fragment for the Steve Bluetooth bridge.
-- Install to ~/.config/wireplumber/bluetooth.lua.d/51-steve-bluez.lua
--
-- Loaded after the system 50-bluez-config.lua (which defines
-- bluez_monitor.properties) and before 90-enable-all.lua, so it only has to
-- override the keys we care about.

-- Let oFono own the HFP/HSP profiles. PipeWire then registers a
-- HandsfreeAudioAgent with oFono and carries the SCO audio, while oFono
-- exposes ring / caller id / answer / hangup on org.ofono.VoiceCallManager.
-- (The native backend, PipeWire 1.0.5, has no call-control API.)
bluez_monitor.properties["bluez5.hfphsp-backend"] = "ofono"

-- Laptop acts as the hands-free unit (phone is the audio gateway).
-- Keep A2DP so normal headphones still work.
bluez_monitor.properties["bluez5.roles"] = "[ hfp_hf a2dp_sink a2dp_source ]"

-- Offer only CVSD (8 kHz) to the phone. With mSBC enabled the Pixel picks it,
-- but this controller (Intel AX211, no "wideband-speech" in its settings)
-- cannot set up the transparent eSCO link: oFono hands PipeWire an
-- unconnected socket ("setsockopt(SCO_OPTIONS): Transport endpoint is not
-- connected") and the audio nodes are torn down immediately. CVSD is also
-- exactly the 8 kHz format the Steve pipeline uses.
bluez_monitor.properties["bluez5.enable-msbc"] = false
