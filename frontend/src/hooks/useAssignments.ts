import { useEffect, useState, useCallback } from "react";
import type { Assignment, CreateAssignmentRequest } from "../types";

const API_BASE = import.meta.env.VITE_API_BASE_URL || "";

export function useAssignments(token: string | null) {
  const [assignments, setAssignments] = useState<Assignment[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const fetchAssignments = useCallback(async () => {
    if (!token) return;
    setError(null);
    try {
      const res = await fetch(`${API_BASE}/api/assignments`, {
        headers: { Authorization: `Bearer ${token}` },
      });
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      const data: Assignment[] = await res.json();
      setAssignments(data);
    } catch (e) {
      setError(e instanceof Error ? e.message : "Unknown error");
    } finally {
      setLoading(false);
    }
  }, [token]);

  useEffect(() => {
    if (!token) return;
    fetchAssignments();
    const interval = setInterval(fetchAssignments, 15_000);
    return () => clearInterval(interval);
  }, [token, fetchAssignments]);

  return { assignments, loading, error, refetch: fetchAssignments };
}

export function useAssignment(token: string | null, id: string | null) {
  const [assignment, setAssignment] = useState<Assignment | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    if (!token || !id) return;
    let cancelled = false;

    async function load() {
      setLoading(true);
      setError(null);
      try {
        const res = await fetch(`${API_BASE}/api/assignments/${id}`, {
          headers: { Authorization: `Bearer ${token}` },
        });
        if (!res.ok) throw new Error(`HTTP ${res.status}`);
        const data: Assignment = await res.json();
        if (!cancelled) setAssignment(data);
      } catch (e) {
        if (!cancelled) setError(e instanceof Error ? e.message : "Unknown error");
      } finally {
        if (!cancelled) setLoading(false);
      }
    }

    load();
    return () => { cancelled = true; };
  }, [token, id]);

  const save = useCallback(async (updated: Assignment) => {
    if (!token || !id) return;
    setSaving(true);
    setError(null);
    try {
      const res = await fetch(`${API_BASE}/api/assignments/${id}`, {
        method: "PUT",
        headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json" },
        body: JSON.stringify(updated),
      });
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      setAssignment(updated);
    } catch (e) {
      setError(e instanceof Error ? e.message : "Unknown error");
      throw e;
    } finally {
      setSaving(false);
    }
  }, [token, id]);

  const remove = useCallback(async () => {
    if (!token || !id) return;
    const res = await fetch(`${API_BASE}/api/assignments/${id}`, {
      method: "DELETE",
      headers: { Authorization: `Bearer ${token}` },
    });
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
  }, [token, id]);

  return { assignment, loading, error, saving, save, remove };
}

export async function createAssignment(
  token: string,
  data: CreateAssignmentRequest,
): Promise<string> {
  const res = await fetch(`${API_BASE}/api/assignments`, {
    method: "POST",
    headers: {
      Authorization: `Bearer ${token}`,
      "Content-Type": "application/json",
    },
    body: JSON.stringify(data),
  });
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  const result = await res.json();
  return result.id;
}
