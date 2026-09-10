import { useState, useEffect } from "react";
import type { AgentProfile } from "../types";
import { useProfile } from "../hooks/useProfiles";

interface Props {
  token: string;
  profileId: string;
  onBack: () => void;
}

export function ProfileEditor({ token, profileId, onBack }: Props) {
  const { profile, loading, error, saving, save } = useProfile(token, profileId);
  const [form, setForm] = useState<AgentProfile | null>(null);
  const [saved, setSaved] = useState(false);

  useEffect(() => {
    if (profile) setForm({ ...profile });
  }, [profile]);

  if (loading || !form) {
    return <div className="py-8 text-center text-gray-400">Loading profile...</div>;
  }

  if (error && !form) {
    return (
      <div className="rounded-md bg-red-50 p-4 text-red-700">
        Failed to load profile: {error}
      </div>
    );
  }

  function updateField(
    field: keyof AgentProfile,
    value: string | number | number[] | null,
  ) {
    setForm((prev) => (prev ? { ...prev, [field]: value } : prev));
    setSaved(false);
  }

  function toggleWorkingDay(day: number) {
    setForm((prev) => {
      if (!prev) return prev;
      const has = prev.working_days.includes(day);
      const next = has
        ? prev.working_days.filter((d) => d !== day)
        : [...prev.working_days, day].sort((a, b) => a - b);
      return { ...prev, working_days: next };
    });
    setSaved(false);
  }

  async function handleSave() {
    if (!form) return;
    try {
      await save(form);
      setSaved(true);
      setTimeout(() => setSaved(false), 3000);
    } catch {
      // error is set by the hook
    }
  }

  return (
    <div className="space-y-6">
      <div className="flex items-center gap-4">
        <button
          onClick={onBack}
          className="text-sm text-blue-600 hover:text-blue-800 cursor-pointer"
        >
          &larr; Back to profiles
        </button>
        <h2 className="text-lg font-semibold text-gray-800">
          Profile: {profileId}
        </h2>
      </div>

      <div className="rounded-lg bg-white p-6 shadow space-y-5">
        <div className="grid grid-cols-2 gap-4">
          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">
              Model
            </label>
            <input
              type="text"
              value={form.model}
              onChange={(e) => updateField("model", e.target.value)}
              className="w-full rounded-md border border-gray-300 px-3 py-2 text-sm"
            />
          </div>
          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">
              Max Tokens
            </label>
            <input
              type="number"
              value={form.max_tokens}
              onChange={(e) => updateField("max_tokens", parseInt(e.target.value) || 0)}
              className="w-full rounded-md border border-gray-300 px-3 py-2 text-sm"
            />
          </div>
        </div>

        <div>
          <label className="block text-sm font-medium text-gray-700 mb-1">
            ElevenLabs Voice ID
            <span className="ml-2 text-xs text-gray-400">
              Leave empty to use the server default (ELEVENLABS_VOICE_ID)
            </span>
          </label>
          <input
            type="text"
            value={form.voice_id ?? ""}
            onChange={(e) => updateField("voice_id", e.target.value || null)}
            placeholder="e.g. 21m00Tcm4TlvDq8ikWAM"
            className="w-full rounded-md border border-gray-300 px-3 py-2 text-sm font-mono"
          />
        </div>

        <div>
          <label className="block text-sm font-medium text-gray-700 mb-1">
            Base Prompt
          </label>
          <textarea
            rows={5}
            value={form.base_prompt}
            onChange={(e) => updateField("base_prompt", e.target.value)}
            className="w-full rounded-md border border-gray-300 px-3 py-2 text-sm font-mono"
          />
        </div>

        <div>
          <label className="block text-sm font-medium text-gray-700 mb-1">
            New Caller Prompt
            <span className="ml-2 text-xs text-gray-400">
              Placeholders: {"{phone}"}
            </span>
          </label>
          <textarea
            rows={4}
            value={form.new_caller_prompt}
            onChange={(e) => updateField("new_caller_prompt", e.target.value)}
            className="w-full rounded-md border border-gray-300 px-3 py-2 text-sm font-mono"
          />
        </div>

        <div>
          <label className="block text-sm font-medium text-gray-700 mb-1">
            Returning Caller Prompt
            <span className="ml-2 text-xs text-gray-400">
              Placeholders: {"{phone}"}, {"{name}"}
            </span>
          </label>
          <textarea
            rows={4}
            value={form.returning_caller_prompt}
            onChange={(e) => updateField("returning_caller_prompt", e.target.value)}
            className="w-full rounded-md border border-gray-300 px-3 py-2 text-sm font-mono"
          />
        </div>

        <div>
          <label className="block text-sm font-medium text-gray-700 mb-1">
            Memory Prompt
            <span className="ml-2 text-xs text-gray-400">
              Placeholders: {"{name}"}, {"{memories}"}
            </span>
          </label>
          <textarea
            rows={4}
            value={form.memory_prompt}
            onChange={(e) => updateField("memory_prompt", e.target.value)}
            className="w-full rounded-md border border-gray-300 px-3 py-2 text-sm font-mono"
          />
        </div>

        <div>
          <label className="block text-sm font-medium text-gray-700 mb-1">
            Outbound Call Prompt
            <span className="ml-2 text-xs text-gray-400">
              Placeholders: {"{name}"}, {"{phone}"}, {"{objective}"}
            </span>
          </label>
          <textarea
            rows={5}
            value={form.outbound_prompt}
            onChange={(e) => updateField("outbound_prompt", e.target.value)}
            className="w-full rounded-md border border-gray-300 px-3 py-2 text-sm font-mono"
          />
        </div>

        <div className="border-t border-gray-200 pt-5">
          <h3 className="text-sm font-semibold text-gray-800 mb-3">
            Calendar & Scheduling
          </h3>

          <div className="grid grid-cols-3 gap-4">
            <div>
              <label className="block text-sm font-medium text-gray-700 mb-1">
                Timezone
              </label>
              <input
                type="text"
                list="tz-suggestions"
                value={form.timezone}
                onChange={(e) => updateField("timezone", e.target.value)}
                placeholder="America/Los_Angeles"
                className="w-full rounded-md border border-gray-300 px-3 py-2 text-sm"
              />
              <datalist id="tz-suggestions">
                <option value="America/Los_Angeles" />
                <option value="America/Denver" />
                <option value="America/Chicago" />
                <option value="America/New_York" />
                <option value="Europe/London" />
                <option value="Europe/Berlin" />
                <option value="Europe/Amsterdam" />
                <option value="Europe/Paris" />
                <option value="Europe/Warsaw" />
                <option value="Asia/Dubai" />
                <option value="Asia/Singapore" />
                <option value="Asia/Tokyo" />
                <option value="Australia/Sydney" />
                <option value="UTC" />
              </datalist>
              <p className="mt-1 text-xs text-gray-400">IANA name</p>
            </div>

            <div>
              <label className="block text-sm font-medium text-gray-700 mb-1">
                Working hours start
              </label>
              <input
                type="number"
                min={0}
                max={23}
                value={form.working_hours_start}
                onChange={(e) =>
                  updateField(
                    "working_hours_start",
                    Math.max(0, Math.min(23, parseInt(e.target.value) || 0)),
                  )
                }
                className="w-full rounded-md border border-gray-300 px-3 py-2 text-sm"
              />
              <p className="mt-1 text-xs text-gray-400">hour 0–23</p>
            </div>

            <div>
              <label className="block text-sm font-medium text-gray-700 mb-1">
                Working hours end
              </label>
              <input
                type="number"
                min={1}
                max={24}
                value={form.working_hours_end}
                onChange={(e) =>
                  updateField(
                    "working_hours_end",
                    Math.max(1, Math.min(24, parseInt(e.target.value) || 0)),
                  )
                }
                className="w-full rounded-md border border-gray-300 px-3 py-2 text-sm"
              />
              <p className="mt-1 text-xs text-gray-400">hour 1–24</p>
            </div>
          </div>

          <div className="mt-4">
            <label className="block text-sm font-medium text-gray-700 mb-2">
              Working days
            </label>
            <div className="flex gap-2 flex-wrap">
              {[
                { num: 1, label: "Mon" },
                { num: 2, label: "Tue" },
                { num: 3, label: "Wed" },
                { num: 4, label: "Thu" },
                { num: 5, label: "Fri" },
                { num: 6, label: "Sat" },
                { num: 7, label: "Sun" },
              ].map(({ num, label }) => {
                const active = form.working_days.includes(num);
                return (
                  <button
                    key={num}
                    type="button"
                    onClick={() => toggleWorkingDay(num)}
                    className={`rounded-md border px-3 py-1.5 text-sm cursor-pointer ${
                      active
                        ? "bg-blue-600 border-blue-600 text-white"
                        : "bg-white border-gray-300 text-gray-700 hover:bg-gray-50"
                    }`}
                  >
                    {label}
                  </button>
                );
              })}
            </div>
          </div>

          <div className="mt-4">
            <label className="block text-sm font-medium text-gray-700 mb-1">
              Calendar Prompt
              <span className="ml-2 text-xs text-gray-400">
                Extra scheduling rules injected when calendar tools are active
                (leave empty to use built-in defaults)
              </span>
            </label>
            <textarea
              rows={4}
              value={form.calendar_prompt}
              onChange={(e) => updateField("calendar_prompt", e.target.value)}
              className="w-full rounded-md border border-gray-300 px-3 py-2 text-sm font-mono"
            />
          </div>
        </div>

        {error && (
          <div className="rounded-md bg-red-50 p-3 text-sm text-red-700">
            Failed to save: {error}
          </div>
        )}

        <div className="flex items-center gap-4">
          <button
            onClick={handleSave}
            disabled={saving}
            className="rounded-md bg-blue-600 px-5 py-2 text-sm font-medium text-white hover:bg-blue-700 disabled:opacity-50 cursor-pointer"
          >
            {saving ? "Saving..." : "Save"}
          </button>
          {saved && (
            <span className="text-sm text-green-600">Saved successfully</span>
          )}
        </div>
      </div>
    </div>
  );
}
