import { useState } from "react";
import type { CalendarAccountStatus, CalendarConfigStatus } from "../types";

interface Props {
  status: CalendarConfigStatus | null;
  loading: boolean;
  error: string | null;
  onReauth: (label: string) => Promise<void>;
  onRefresh: () => void;
}

export function CalendarStatusBanner({ status, loading, error, onReauth, onRefresh }: Props) {
  const [busyLabel, setBusyLabel] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);

  if (loading || !status) return null;
  if (!status.configured) return null;

  const broken = status.accounts.filter((a) => !a.valid);
  const allValid = broken.length === 0 && status.accounts.length > 0;

  if (status.accounts.length === 0) {
    return (
      <div className="mx-auto max-w-7xl px-6 pt-4">
        <div className="rounded-md border border-amber-200 bg-amber-50 px-4 py-3 text-sm text-amber-800 flex items-center justify-between">
          <span>No calendar account connected. Steve can't book meetings.</span>
          <button
            onClick={() => handleReauth("personal")}
            disabled={busyLabel !== null}
            className="rounded-md bg-amber-600 px-3 py-1.5 text-sm font-medium text-white hover:bg-amber-700 disabled:opacity-50 cursor-pointer"
          >
            {busyLabel === "personal" ? "Redirecting..." : "Connect personal calendar"}
          </button>
        </div>
      </div>
    );
  }

  if (allValid) {
    return (
      <div className="mx-auto max-w-7xl px-6 pt-4">
        <div className="rounded-md border border-green-200 bg-green-50 px-4 py-2 text-xs text-green-800 flex items-center justify-between">
          <span>
            Calendar connected:{" "}
            {status.accounts
              .map((a) => `${a.label}${a.account_email ? ` (${a.account_email})` : ""}`)
              .join(", ")}
          </span>
          <button
            onClick={onRefresh}
            className="text-xs text-green-700 hover:text-green-900 cursor-pointer"
          >
            Re-check
          </button>
        </div>
      </div>
    );
  }

  async function handleReauth(label: string) {
    setBusyLabel(label);
    setActionError(null);
    try {
      await onReauth(label);
    } catch (e) {
      setActionError(e instanceof Error ? e.message : "Failed to start reauth");
      setBusyLabel(null);
    }
  }

  return (
    <div className="mx-auto max-w-7xl px-6 pt-4 space-y-2">
      {error && (
        <div className="rounded-md bg-red-50 px-4 py-2 text-xs text-red-700">
          Failed to check calendar status: {error}
        </div>
      )}
      {broken.map((acc: CalendarAccountStatus) => (
        <div
          key={acc.label}
          className="rounded-md border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-800"
        >
          <div className="flex items-center justify-between gap-4">
            <div className="min-w-0">
              <div className="font-medium">
                Calendar "{acc.label}"
                {acc.account_email && (
                  <span className="font-normal text-red-700"> — {acc.account_email}</span>
                )}{" "}
                is disconnected
              </div>
              {acc.error && (
                <div className="mt-1 truncate font-mono text-xs text-red-700" title={acc.error}>
                  {acc.error}
                </div>
              )}
            </div>
            <button
              onClick={() => handleReauth(acc.label)}
              disabled={busyLabel !== null}
              className="shrink-0 rounded-md bg-red-600 px-3 py-1.5 text-sm font-medium text-white hover:bg-red-700 disabled:opacity-50 cursor-pointer"
            >
              {busyLabel === acc.label ? "Redirecting..." : "Reconnect"}
            </button>
          </div>
        </div>
      ))}
      {actionError && (
        <div className="rounded-md bg-red-50 px-4 py-2 text-xs text-red-700">
          {actionError}
        </div>
      )}
    </div>
  );
}
