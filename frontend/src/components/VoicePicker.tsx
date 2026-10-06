import { useRef, useState } from "react";
import type { Voice } from "../types";
import { useVoices } from "../hooks/useVoices";

interface Props {
  token: string;
  /** The profile's voice id; null means the server default. */
  value: string | null;
  onChange: (voiceId: string | null) => void;
}

const DEFAULT_OPTION = "";
const CUSTOM_OPTION = "__custom__";

function describe(v: Voice): string {
  const tags = ["gender", "accent", "age", "use_case"]
    .map((k) => v.labels[k])
    .filter(Boolean);
  const detail = [v.category, ...tags].filter(Boolean).join(", ");
  return detail ? `${v.name} — ${detail}` : v.name;
}

/**
 * ElevenLabs voice selector: the account's voices from GET /api/voices with a
 * preview button, plus a manual-ID escape hatch for voices not in the list or
 * when the list cannot be loaded.
 */
export function VoicePicker({ token, value, onChange }: Props) {
  const { voices, loading, error } = useVoices(token);
  // The user asked for the manual field; a saved id missing from the loaded
  // list also lands there, since there is nothing to select it with.
  const [customChosen, setCustomChosen] = useState(false);
  const audioRef = useRef<HTMLAudioElement | null>(null);

  const inList = value !== null && voices.some((v) => v.voice_id === value);
  const custom = customChosen || (!loading && value !== null && !inList);

  const selected = voices.find((v) => v.voice_id === value) ?? null;

  function handleSelect(option: string) {
    audioRef.current?.pause();
    if (option === CUSTOM_OPTION) {
      setCustomChosen(true);
      return;
    }
    setCustomChosen(false);
    onChange(option === DEFAULT_OPTION ? null : option);
  }

  function playPreview() {
    if (!selected?.preview_url) return;
    const audio = audioRef.current ?? new Audio();
    audioRef.current = audio;
    audio.src = selected.preview_url;
    audio.play().catch(() => {
      /* autoplay restrictions: the user can press again */
    });
  }

  const manualInput = (
    <input
      type="text"
      value={value ?? ""}
      onChange={(e) => onChange(e.target.value || null)}
      placeholder="e.g. 21m00Tcm4TlvDq8ikWAM"
      className="w-full rounded-md border border-gray-300 px-3 py-2 text-sm font-mono"
    />
  );

  if (error) {
    return (
      <div>
        <label className="block text-sm font-medium text-gray-700 mb-1">
          ElevenLabs Voice ID
          <span className="ml-2 text-xs text-red-500">
            Voice list unavailable ({error}); enter an ID
          </span>
        </label>
        {manualInput}
      </div>
    );
  }

  const selectValue = custom ? CUSTOM_OPTION : value ?? DEFAULT_OPTION;

  return (
    <div>
      <label className="block text-sm font-medium text-gray-700 mb-1">
        Voice
        <span className="ml-2 text-xs text-gray-400">
          ElevenLabs voice this agent speaks with
        </span>
      </label>
      <div className="flex gap-2">
        <select
          value={selectValue}
          onChange={(e) => handleSelect(e.target.value)}
          disabled={loading}
          className="w-full rounded-md border border-gray-300 px-3 py-2 text-sm"
        >
          <option value={DEFAULT_OPTION}>
            Server default (ELEVENLABS_VOICE_ID)
          </option>
          {voices.map((v) => (
            <option key={v.voice_id} value={v.voice_id}>
              {describe(v)}
            </option>
          ))}
          <option value={CUSTOM_OPTION}>Custom voice ID…</option>
        </select>
        <button
          type="button"
          onClick={playPreview}
          disabled={!selected?.preview_url}
          title={selected?.preview_url ? "Play a sample" : "No preview for this voice"}
          className="shrink-0 rounded-md border border-gray-300 px-3 py-2 text-sm text-gray-700 hover:bg-gray-50 disabled:opacity-40 disabled:cursor-not-allowed cursor-pointer"
        >
          ▶ Preview
        </button>
      </div>
      {loading && (
        <p className="mt-1 text-xs text-gray-400">Loading voices…</p>
      )}
      {custom && <div className="mt-2">{manualInput}</div>}
    </div>
  );
}
