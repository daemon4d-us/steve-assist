import type { SessionSummary } from "../types";

interface Props {
  sessions: SessionSummary[];
  loading: boolean;
  error: string | null;
}

function formatDuration(seconds: number | null): string {
  if (seconds === null) return "In progress";
  const m = Math.floor(seconds / 60);
  const s = seconds % 60;
  return m > 0 ? `${m}m ${s}s` : `${s}s`;
}

function formatTime(iso: string): string {
  return new Date(iso).toLocaleString();
}

export function SessionsTable({ sessions, loading, error }: Props) {
  if (error) {
    return (
      <div className="rounded-md bg-red-50 p-4 text-red-700">
        Failed to load sessions: {error}
      </div>
    );
  }

  return (
    <div className="overflow-x-auto rounded-lg bg-white shadow">
      <table className="min-w-full divide-y divide-gray-200">
        <thead className="bg-gray-50">
          <tr>
            <th className="px-6 py-3 text-left text-xs font-medium uppercase tracking-wider text-gray-500">
              Date/Time
            </th>
            <th className="px-6 py-3 text-left text-xs font-medium uppercase tracking-wider text-gray-500">
              Direction
            </th>
            <th className="px-6 py-3 text-left text-xs font-medium uppercase tracking-wider text-gray-500">
              Caller
            </th>
            <th className="px-6 py-3 text-left text-xs font-medium uppercase tracking-wider text-gray-500">
              Duration
            </th>
            <th className="px-6 py-3 text-left text-xs font-medium uppercase tracking-wider text-gray-500">
              Summary
            </th>
          </tr>
        </thead>
        <tbody className="divide-y divide-gray-200">
          {loading && sessions.length === 0 ? (
            <tr>
              <td colSpan={5} className="px-6 py-8 text-center text-gray-400">
                Loading...
              </td>
            </tr>
          ) : sessions.length === 0 ? (
            <tr>
              <td colSpan={5} className="px-6 py-8 text-center text-gray-400">
                No call sessions yet
              </td>
            </tr>
          ) : (
            sessions.map((s) => (
              <tr key={s.call_sid} className="even:bg-gray-50">
                <td className="whitespace-nowrap px-6 py-4 text-sm text-gray-900">
                  {formatTime(s.started_at)}
                </td>
                <td className="px-6 py-4 text-sm">
                  <span className={`inline-block rounded-full px-2 py-0.5 text-xs font-medium ${
                    s.direction === "outbound"
                      ? "bg-blue-100 text-blue-700"
                      : "bg-gray-100 text-gray-700"
                  }`}>
                    {s.direction || "inbound"}
                  </span>
                </td>
                <td className="px-6 py-4 text-sm text-gray-900">
                  <div>{s.caller_phone}</div>
                  {s.caller_name && (
                    <div className="text-xs text-gray-500">{s.caller_name}</div>
                  )}
                </td>
                <td className="whitespace-nowrap px-6 py-4 text-sm text-gray-600">
                  {formatDuration(s.duration_seconds)}
                </td>
                <td className="max-w-md px-6 py-4 text-sm text-gray-600">
                  {s.summary || "—"}
                </td>
              </tr>
            ))
          )}
        </tbody>
      </table>
    </div>
  );
}
