import { useEffect, useState, useCallback } from "react";
import type { CalendarConfigStatus } from "../types";

const API_BASE = import.meta.env.VITE_API_BASE_URL || "";

export function useCalendarStatus(token: string | null) {
  const [status, setStatus] = useState<CalendarConfigStatus | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [reloadTick, setReloadTick] = useState(0);

  useEffect(() => {
    if (!token) return;
    let cancelled = false;

    async function fetch() {
      setLoading(true);
      setError(null);
      try {
        const res = await window.fetch(`${API_BASE}/api/calendar/status`, {
          headers: { Authorization: `Bearer ${token}` },
        });
        if (!res.ok) throw new Error(`HTTP ${res.status}`);
        const data: CalendarConfigStatus = await res.json();
        if (!cancelled) setStatus(data);
      } catch (e) {
        if (!cancelled) setError(e instanceof Error ? e.message : "Unknown error");
      } finally {
        if (!cancelled) setLoading(false);
      }
    }

    fetch();
    return () => { cancelled = true; };
  }, [token, reloadTick]);

  const refresh = useCallback(() => setReloadTick((n) => n + 1), []);

  const reauth = useCallback(async (label: string) => {
    if (!token) return;
    const res = await window.fetch(`${API_BASE}/api/calendar/reauth`, {
      method: "POST",
      headers: {
        Authorization: `Bearer ${token}`,
        "Content-Type": "application/json",
      },
      body: JSON.stringify({ label }),
    });
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const { url } = (await res.json()) as { url: string };
    window.location.href = url;
  }, [token]);

  return { status, loading, error, refresh, reauth };
}
