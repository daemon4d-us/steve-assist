import { useEffect, useState } from "react";
import type { Voice } from "../types";

const API_BASE = import.meta.env.VITE_API_BASE_URL || "";

/** The ElevenLabs voices the account can use, for the profile voice picker. */
export function useVoices(token: string | null) {
  const [voices, setVoices] = useState<Voice[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!token) return;
    let cancelled = false;

    async function fetch() {
      setLoading(true);
      setError(null);
      try {
        const res = await window.fetch(`${API_BASE}/api/voices`, {
          headers: { Authorization: `Bearer ${token}` },
        });
        if (!res.ok) {
          // The server sends a plain-text reason for 502s (e.g. a key without voices_read).
          const reason = res.status === 502 ? (await res.text()).trim() : "";
          throw new Error(reason || `HTTP ${res.status}`);
        }
        const data: Voice[] = await res.json();
        if (!cancelled) setVoices(data);
      } catch (e) {
        if (!cancelled) setError(e instanceof Error ? e.message : "Unknown error");
      } finally {
        if (!cancelled) setLoading(false);
      }
    }

    fetch();
    return () => { cancelled = true; };
  }, [token]);

  return { voices, loading, error };
}
