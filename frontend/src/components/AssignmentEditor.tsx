import { useState, useEffect } from "react";
import type { ProfileListItem, CreateAssignmentRequest } from "../types";
import { useAssignment, createAssignment } from "../hooks/useAssignments";

interface Props {
  token: string;
  assignmentId: string | null; // null = creating new
  profiles: ProfileListItem[];
  onBack: () => void;
}

interface ContactInput {
  phone: string;
  name: string;
}

export function AssignmentEditor({ token, assignmentId, profiles, onBack }: Props) {
  const { assignment, loading, error, saving, save, remove } = useAssignment(
    token,
    assignmentId
  );

  const [objective, setObjective] = useState("");
  const [profileId, setProfileId] = useState(profiles[0]?.id || "default");
  const [scheduledAt, setScheduledAt] = useState("");
  const [contacts, setContacts] = useState<ContactInput[]>([{ phone: "", name: "" }]);
  const [submitError, setSubmitError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);

  useEffect(() => {
    if (assignment) {
      setObjective(assignment.objective);
      setProfileId(assignment.profile_id);
      setScheduledAt(assignment.scheduled_at.slice(0, 16)); // datetime-local format
      setContacts(
        assignment.contacts.map((c) => ({
          phone: c.phone,
          name: c.name || "",
        }))
      );
    }
  }, [assignment]);

  function addContact() {
    setContacts([...contacts, { phone: "", name: "" }]);
  }

  function removeContact(idx: number) {
    setContacts(contacts.filter((_, i) => i !== idx));
  }

  function updateContact(idx: number, field: "phone" | "name", value: string) {
    setContacts(
      contacts.map((c, i) => (i === idx ? { ...c, [field]: value } : c))
    );
  }

  async function handleSave() {
    setSubmitError(null);
    setSaved(false);

    const validContacts = contacts.filter((c) => c.phone.trim());
    if (!objective.trim() || !scheduledAt || validContacts.length === 0) {
      setSubmitError("Objective, scheduled time, and at least one contact are required.");
      return;
    }

    try {
      if (assignmentId && assignment) {
        // Update existing
        await save({
          ...assignment,
          objective,
          profile_id: profileId,
          scheduled_at: new Date(scheduledAt).toISOString(),
          contacts: assignment.contacts.map((c, i) => ({
            ...c,
            phone: validContacts[i]?.phone || c.phone,
            name: validContacts[i]?.name || c.name,
          })),
        });
      } else {
        // Create new
        const data: CreateAssignmentRequest = {
          objective,
          profile_id: profileId,
          scheduled_at: new Date(scheduledAt).toISOString(),
          contacts: validContacts.map((c) => ({
            phone: c.phone.trim(),
            name: c.name.trim() || undefined,
          })),
        };
        await createAssignment(token, data);
      }
      setSaved(true);
      setTimeout(() => {
        setSaved(false);
        if (!assignmentId) onBack();
      }, 1500);
    } catch (e) {
      setSubmitError(e instanceof Error ? e.message : "Unknown error");
    }
  }

  async function handleDelete() {
    if (!confirm("Delete this assignment?")) return;
    try {
      await remove();
      onBack();
    } catch (e) {
      setSubmitError(e instanceof Error ? e.message : "Unknown error");
    }
  }

  if (assignmentId && loading) {
    return <div className="py-8 text-center text-gray-400">Loading assignment...</div>;
  }

  return (
    <div className="space-y-6">
      <div className="flex items-center gap-4">
        <button onClick={onBack} className="text-sm text-blue-600 hover:text-blue-800 cursor-pointer">
          &larr; Back to assignments
        </button>
        <h2 className="text-lg font-semibold text-gray-800">
          {assignmentId ? "Edit Assignment" : "New Assignment"}
        </h2>
      </div>

      <div className="rounded-lg bg-white p-6 shadow space-y-5">
        <div className="grid grid-cols-2 gap-4">
          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">Scheduled Date & Time</label>
            <input
              type="datetime-local"
              value={scheduledAt}
              onChange={(e) => setScheduledAt(e.target.value)}
              className="w-full rounded-md border border-gray-300 px-3 py-2 text-sm"
            />
          </div>
          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">Agent Profile</label>
            <select
              value={profileId}
              onChange={(e) => setProfileId(e.target.value)}
              className="w-full rounded-md border border-gray-300 px-3 py-2 text-sm"
            >
              {profiles.map((p) => (
                <option key={p.id} value={p.id}>{p.id} ({p.model})</option>
              ))}
            </select>
          </div>
        </div>

        <div>
          <label className="block text-sm font-medium text-gray-700 mb-1">Objective</label>
          <textarea
            rows={3}
            value={objective}
            onChange={(e) => setObjective(e.target.value)}
            placeholder="Describe what the agent should accomplish on these calls..."
            className="w-full rounded-md border border-gray-300 px-3 py-2 text-sm"
          />
        </div>

        <div>
          <div className="flex items-center justify-between mb-2">
            <label className="block text-sm font-medium text-gray-700">Contacts</label>
            <button
              onClick={addContact}
              className="text-sm text-blue-600 hover:text-blue-800 cursor-pointer"
            >
              + Add contact
            </button>
          </div>
          <div className="space-y-2">
            {contacts.map((c, idx) => (
              <div key={idx} className="flex gap-2 items-center">
                <input
                  type="tel"
                  placeholder="Phone number"
                  value={c.phone}
                  onChange={(e) => updateContact(idx, "phone", e.target.value)}
                  className="flex-1 rounded-md border border-gray-300 px-3 py-2 text-sm"
                />
                <input
                  type="text"
                  placeholder="Name (optional)"
                  value={c.name}
                  onChange={(e) => updateContact(idx, "name", e.target.value)}
                  className="flex-1 rounded-md border border-gray-300 px-3 py-2 text-sm"
                />
                {contacts.length > 1 && (
                  <button
                    onClick={() => removeContact(idx)}
                    className="text-sm text-red-500 hover:text-red-700 cursor-pointer"
                  >
                    Remove
                  </button>
                )}
              </div>
            ))}
          </div>
        </div>

        {/* Contact status table for existing assignments */}
        {assignmentId && assignment && (
          <div>
            <label className="block text-sm font-medium text-gray-700 mb-2">Contact Status</label>
            <div className="overflow-x-auto rounded border border-gray-200">
              <table className="min-w-full divide-y divide-gray-200 text-sm">
                <thead className="bg-gray-50">
                  <tr>
                    <th className="px-4 py-2 text-left text-xs font-medium text-gray-500">Phone</th>
                    <th className="px-4 py-2 text-left text-xs font-medium text-gray-500">Name</th>
                    <th className="px-4 py-2 text-left text-xs font-medium text-gray-500">Status</th>
                    <th className="px-4 py-2 text-left text-xs font-medium text-gray-500">Attempts</th>
                    <th className="px-4 py-2 text-left text-xs font-medium text-gray-500">Error</th>
                  </tr>
                </thead>
                <tbody className="divide-y divide-gray-200">
                  {assignment.contacts.map((c, idx) => (
                    <tr key={idx} className="even:bg-gray-50">
                      <td className="px-4 py-2">{c.phone}</td>
                      <td className="px-4 py-2">{c.name || "—"}</td>
                      <td className="px-4 py-2">
                        <span className={`inline-block rounded-full px-2 py-0.5 text-xs font-medium ${
                          c.status === "completed" ? "bg-green-100 text-green-700" :
                          c.status === "failed" ? "bg-red-100 text-red-700" :
                          c.status === "in_progress" ? "bg-amber-100 text-amber-700" :
                          "bg-gray-100 text-gray-700"
                        }`}>
                          {c.status}
                        </span>
                      </td>
                      <td className="px-4 py-2">{c.attempts}</td>
                      <td className="px-4 py-2 text-red-600">{c.error || "—"}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </div>
        )}

        {(submitError || error) && (
          <div className="rounded-md bg-red-50 p-3 text-sm text-red-700">
            {submitError || error}
          </div>
        )}

        <div className="flex items-center gap-4">
          <button
            onClick={handleSave}
            disabled={saving}
            className="rounded-md bg-blue-600 px-5 py-2 text-sm font-medium text-white hover:bg-blue-700 disabled:opacity-50 cursor-pointer"
          >
            {saving ? "Saving..." : assignmentId ? "Save" : "Create"}
          </button>
          {assignmentId && (
            <button
              onClick={handleDelete}
              className="rounded-md bg-red-600 px-5 py-2 text-sm font-medium text-white hover:bg-red-700 cursor-pointer"
            >
              Delete
            </button>
          )}
          {saved && <span className="text-sm text-green-600">Saved successfully</span>}
        </div>
      </div>
    </div>
  );
}
