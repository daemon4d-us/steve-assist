import { useEffect, useState, useCallback } from "react";
import type { ProfileListItem, AgentProfile } from "../types";

const API_BASE = import.meta.env.VITE_API_BASE_URL || "";

export function useProfiles(token: string | null) {
  const [profiles, setProfiles] = useState<ProfileListItem[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!token) return;
    let cancelled = false;

    async function fetch() {
      setLoading(true);
      setError(null);
      try {
        const res = await window.fetch(`${API_BASE}/api/profiles`, {
          headers: { Authorization: `Bearer ${token}` },
        });
        if (!res.ok) throw new Error(`HTTP ${res.status}`);
        const data: ProfileListItem[] = await res.json();
        if (!cancelled) setProfiles(data);
      } catch (e) {
        if (!cancelled) setError(e instanceof Error ? e.message : "Unknown error");
      } finally {
        if (!cancelled) setLoading(false);
      }
    }

    fetch();
    return () => { cancelled = true; };
  }, [token]);

  return { profiles, loading, error };
}

export function useProfile(token: string | null, profileId: string | null) {
  const [profile, setProfile] = useState<AgentProfile | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    if (!token || !profileId) return;
    let cancelled = false;

    async function fetch() {
      setLoading(true);
      setError(null);
      try {
        const res = await window.fetch(`${API_BASE}/api/profiles/${profileId}`, {
          headers: { Authorization: `Bearer ${token}` },
        });
        if (!res.ok) throw new Error(`HTTP ${res.status}`);
        const data: AgentProfile = await res.json();
        if (!cancelled) setProfile(data);
      } catch (e) {
        if (!cancelled) setError(e instanceof Error ? e.message : "Unknown error");
      } finally {
        if (!cancelled) setLoading(false);
      }
    }

    fetch();
    return () => { cancelled = true; };
  }, [token, profileId]);

  const save = useCallback(async (updated: AgentProfile) => {
    if (!token || !profileId) return;
    setSaving(true);
    setError(null);
    try {
      const res = await window.fetch(`${API_BASE}/api/profiles/${profileId}`, {
        method: "PUT",
        headers: {
          Authorization: `Bearer ${token}`,
          "Content-Type": "application/json",
        },
        body: JSON.stringify(updated),
      });
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      setProfile(updated);
    } catch (e) {
      setError(e instanceof Error ? e.message : "Unknown error");
      throw e;
    } finally {
      setSaving(false);
    }
  }, [token, profileId]);

  return { profile, loading, error, saving, save };
}
