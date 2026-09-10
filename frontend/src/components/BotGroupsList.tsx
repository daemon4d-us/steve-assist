import { useState } from "react";
import type { BotCallGroup } from "../types";

interface Props {
  groups: BotCallGroup[];
  loading: boolean;
  error: string | null;
}

function formatTime(iso: string): string {
  return new Date(iso).toLocaleString();
}

export function BotGroupsList({ groups, loading, error }: Props) {
  if (error) {
    return (
      <div className="rounded-md bg-red-50 p-4 text-red-700">
        Failed to load bot groups: {error}
      </div>
    );
  }

  if (loading && groups.length === 0) {
    return <div className="py-8 text-center text-gray-400">Loading...</div>;
  }

  if (groups.length === 0) {
    return (
      <div className="rounded-lg bg-white p-8 text-center text-gray-500 shadow">
        No bot calls clustered yet. Groups appear automatically after Steve
        identifies an automated caller.
      </div>
    );
  }

  return (
    <div className="space-y-4">
      {groups.map((g) => (
        <BotGroupCard key={g.id} group={g} />
      ))}
    </div>
  );
}

function BotGroupCard({ group }: { group: BotCallGroup }) {
  const [expanded, setExpanded] = useState(false);

  return (
    <div className="rounded-lg bg-white p-5 shadow">
      <div className="flex items-start justify-between gap-4">
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-3">
            <h3 className="text-base font-semibold text-gray-900">{group.name}</h3>
            <span className="inline-block rounded-full bg-gray-100 px-2 py-0.5 text-xs text-gray-600">
              {group.call_count} call{group.call_count === 1 ? "" : "s"}
            </span>
            <span className="inline-block rounded-full bg-blue-50 px-2 py-0.5 text-xs text-blue-700">
              {group.phone_numbers.length} number
              {group.phone_numbers.length === 1 ? "" : "s"}
            </span>
          </div>
          <div className="mt-1 text-xs text-gray-500">
            Last seen: {formatTime(group.latest_call_at)}
          </div>
        </div>
      </div>

      {group.latest_summary && (
        <p className="mt-3 text-sm text-gray-700">{group.latest_summary}</p>
      )}

      <div className="mt-4">
        <button
          onClick={() => setExpanded((v) => !v)}
          className="text-sm font-medium text-blue-600 hover:text-blue-800 cursor-pointer"
        >
          {expanded ? "Hide numbers" : `Show ${group.phone_numbers.length} number${group.phone_numbers.length === 1 ? "" : "s"}`}
        </button>
        {expanded && (
          <ul className="mt-2 grid grid-cols-2 gap-x-6 gap-y-1 sm:grid-cols-3">
            {group.phone_numbers.map((p) => (
              <li
                key={p}
                className="font-mono text-xs text-gray-700 truncate"
                title={p}
              >
                {p}
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}
