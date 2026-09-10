import type { Assignment } from "../types";

interface Props {
  assignments: Assignment[];
  loading: boolean;
  error: string | null;
  onSelect: (id: string) => void;
  onNew: () => void;
}

function statusBadge(status: string) {
  const colors: Record<string, string> = {
    pending: "bg-gray-100 text-gray-700",
    in_progress: "bg-amber-100 text-amber-700",
    completed: "bg-green-100 text-green-700",
  };
  return (
    <span className={`inline-block rounded-full px-2 py-0.5 text-xs font-medium ${colors[status] || "bg-gray-100 text-gray-700"}`}>
      {status.replace("_", " ")}
    </span>
  );
}

function contactsProgress(contacts: Assignment["contacts"]) {
  const completed = contacts.filter((c) => c.status === "completed").length;
  return `${completed}/${contacts.length}`;
}

export function AssignmentsList({ assignments, loading, error, onSelect, onNew }: Props) {
  if (error) {
    return (
      <div className="rounded-md bg-red-50 p-4 text-red-700">
        Failed to load assignments: {error}
      </div>
    );
  }

  return (
    <div>
      <div className="mb-4 flex justify-end">
        <button
          onClick={onNew}
          className="rounded-md bg-blue-600 px-4 py-2 text-sm font-medium text-white hover:bg-blue-700 cursor-pointer"
        >
          New Assignment
        </button>
      </div>
      <div className="overflow-x-auto rounded-lg bg-white shadow">
        <table className="min-w-full divide-y divide-gray-200">
          <thead className="bg-gray-50">
            <tr>
              <th className="px-6 py-3 text-left text-xs font-medium uppercase tracking-wider text-gray-500">Status</th>
              <th className="px-6 py-3 text-left text-xs font-medium uppercase tracking-wider text-gray-500">Scheduled</th>
              <th className="px-6 py-3 text-left text-xs font-medium uppercase tracking-wider text-gray-500">Objective</th>
              <th className="px-6 py-3 text-left text-xs font-medium uppercase tracking-wider text-gray-500">Profile</th>
              <th className="px-6 py-3 text-left text-xs font-medium uppercase tracking-wider text-gray-500">Contacts</th>
              <th className="px-6 py-3" />
            </tr>
          </thead>
          <tbody className="divide-y divide-gray-200">
            {loading ? (
              <tr>
                <td colSpan={6} className="px-6 py-8 text-center text-gray-400">Loading...</td>
              </tr>
            ) : assignments.length === 0 ? (
              <tr>
                <td colSpan={6} className="px-6 py-8 text-center text-gray-400">No assignments yet</td>
              </tr>
            ) : (
              assignments.map((a) => (
                <tr key={a.id} className="even:bg-gray-50 cursor-pointer hover:bg-gray-100" onClick={() => onSelect(a.id)}>
                  <td className="px-6 py-4">{statusBadge(a.status)}</td>
                  <td className="whitespace-nowrap px-6 py-4 text-sm text-gray-900">
                    {new Date(a.scheduled_at).toLocaleString()}
                  </td>
                  <td className="max-w-xs truncate px-6 py-4 text-sm text-gray-600">{a.objective}</td>
                  <td className="px-6 py-4 text-sm text-gray-600">{a.profile_id}</td>
                  <td className="px-6 py-4 text-sm text-gray-600">{contactsProgress(a.contacts)}</td>
                  <td className="px-6 py-4 text-right">
                    <span className="text-sm text-blue-600">View</span>
                  </td>
                </tr>
              ))
            )}
          </tbody>
        </table>
      </div>
    </div>
  );
}
