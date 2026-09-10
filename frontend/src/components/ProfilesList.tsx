import type { ProfileListItem } from "../types";

interface Props {
  profiles: ProfileListItem[];
  loading: boolean;
  error: string | null;
  onSelect: (id: string) => void;
}

export function ProfilesList({ profiles, loading, error, onSelect }: Props) {
  if (error) {
    return (
      <div className="rounded-md bg-red-50 p-4 text-red-700">
        Failed to load profiles: {error}
      </div>
    );
  }

  return (
    <div className="overflow-x-auto rounded-lg bg-white shadow">
      <table className="min-w-full divide-y divide-gray-200">
        <thead className="bg-gray-50">
          <tr>
            <th className="px-6 py-3 text-left text-xs font-medium uppercase tracking-wider text-gray-500">
              Profile ID
            </th>
            <th className="px-6 py-3 text-left text-xs font-medium uppercase tracking-wider text-gray-500">
              Model
            </th>
            <th className="px-6 py-3 text-left text-xs font-medium uppercase tracking-wider text-gray-500">
              Max Tokens
            </th>
            <th className="px-6 py-3" />
          </tr>
        </thead>
        <tbody className="divide-y divide-gray-200">
          {loading ? (
            <tr>
              <td colSpan={4} className="px-6 py-8 text-center text-gray-400">
                Loading...
              </td>
            </tr>
          ) : profiles.length === 0 ? (
            <tr>
              <td colSpan={4} className="px-6 py-8 text-center text-gray-400">
                No agent profiles found
              </td>
            </tr>
          ) : (
            profiles.map((p) => (
              <tr key={p.id} className="even:bg-gray-50">
                <td className="px-6 py-4 text-sm font-medium text-gray-900">
                  {p.id}
                </td>
                <td className="px-6 py-4 text-sm text-gray-600">{p.model}</td>
                <td className="px-6 py-4 text-sm text-gray-600">
                  {p.max_tokens}
                </td>
                <td className="px-6 py-4 text-right">
                  <button
                    onClick={() => onSelect(p.id)}
                    className="text-sm text-blue-600 hover:text-blue-800 cursor-pointer"
                  >
                    Edit
                  </button>
                </td>
              </tr>
            ))
          )}
        </tbody>
      </table>
    </div>
  );
}
