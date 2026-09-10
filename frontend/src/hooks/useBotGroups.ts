import { useEffect, useState } from "react";
import type { BotCallGroup } from "../types";

const API_BASE = import.meta.env.VITE_API_BASE_URL || "";

export function useBotGroups(token: string | null) {
  const [groups, setGroups] = useState<BotCallGroup[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!token) return;
    let cancelled = false;

    async function fetch() {
      setLoading(true);
      setError(null);
      try {
        const res = await window.fetch(`${API_BASE}/api/bot-groups`, {
          headers: { Authorization: `Bearer ${token}` },
        });
        if (!res.ok) throw new Error(`HTTP ${res.status}`);
        const data: BotCallGroup[] = await res.json();
        if (!cancelled) setGroups(data);
      } catch (e) {
        if (!cancelled) setError(e instanceof Error ? e.message : "Unknown error");
      } finally {
        if (!cancelled) setLoading(false);
      }
    }

    fetch();
    return () => { cancelled = true; };
  }, [token]);

  return { groups, loading, error };
}
