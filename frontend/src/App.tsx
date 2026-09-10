import { useState, useEffect, useCallback, useRef } from "react";
import { LoginButton } from "./components/LoginButton";
import { SessionsTable } from "./components/SessionsTable";
import { ProfilesList } from "./components/ProfilesList";
import { ProfileEditor } from "./components/ProfileEditor";
import { AssignmentsList } from "./components/AssignmentsList";
import { AssignmentEditor } from "./components/AssignmentEditor";
import { BotGroupsList } from "./components/BotGroupsList";
import { CalendarStatusBanner } from "./components/CalendarStatusBanner";
import { useSessions } from "./hooks/useSessions";
import { useProfiles } from "./hooks/useProfiles";
import { useAssignments } from "./hooks/useAssignments";
import { useBotGroups } from "./hooks/useBotGroups";
import { useCalendarStatus } from "./hooks/useCalendarStatus";

declare global {
  interface Window {
    google?: {
      accounts: {
        oauth2: {
          initTokenClient(config: {
            client_id: string;
            scope: string;
            prompt?: string;
            callback: (response: { access_token?: string; error?: string }) => void;
          }): { requestAccessToken(opts?: { prompt?: string }): void };
        };
      };
    };
  }
}

type Tab = "sessions" | "profiles" | "assignments" | "bots";
const VALID_TABS: Tab[] = ["sessions", "profiles", "assignments", "bots"];

function getTabFromHash(): Tab {
  const hash = window.location.hash.replace("#", "");
  return VALID_TABS.includes(hash as Tab) ? (hash as Tab) : "sessions";
}

export default function App() {
  const [token, setToken] = useState<string | null>(
    localStorage.getItem("google_token")
  );
  const [tab, setTab] = useState<Tab>(getTabFromHash);
  const [editingProfile, setEditingProfile] = useState<string | null>(null);
  const [editingAssignment, setEditingAssignment] = useState<string | null | "new">(null);

  const switchTab = useCallback((t: Tab) => {
    setTab(t);
    setEditingProfile(null);
    setEditingAssignment(null);
    window.location.hash = t;
  }, []);

  useEffect(() => {
    const onHashChange = () => setTab(getTabFromHash());
    window.addEventListener("hashchange", onHashChange);
    return () => window.removeEventListener("hashchange", onHashChange);
  }, []);

  const { sessions, loading: sessionsLoading, error: sessionsError } = useSessions(token);
  const { profiles, loading: profilesLoading, error: profilesError } = useProfiles(token);
  const { assignments, loading: assignmentsLoading, error: assignmentsError } = useAssignments(token);
  const { groups: botGroups, loading: botsLoading, error: botsError } = useBotGroups(token);
  const {
    status: calendarStatus,
    loading: calendarLoading,
    error: calendarError,
    refresh: refreshCalendar,
    reauth: reauthCalendar,
  } = useCalendarStatus(token);

  const refreshingRef = useRef(false);

  useEffect(() => {
    const has401 = [sessionsError, profilesError, assignmentsError, botsError].some(
      (e) => e?.includes("401")
    );
    if (!has401 || refreshingRef.current) return;

    // The GSI script may not have finished loading yet. If so, just logout —
    // the LoginButton flow (via @react-oauth/google) waits for the script.
    if (!window.google?.accounts?.oauth2) {
      handleLogout();
      return;
    }

    refreshingRef.current = true;

    const clientId = import.meta.env.VITE_GOOGLE_CLIENT_ID || "";
    const tokenClient = window.google.accounts.oauth2.initTokenClient({
      client_id: clientId,
      scope: "email profile",
      prompt: "",
      callback: (response) => {
        refreshingRef.current = false;
        if (response.access_token) {
          handleLogin(response.access_token);
        } else {
          handleLogout();
        }
      },
    });
    tokenClient.requestAccessToken({ prompt: "" });
  }, [sessionsError, profilesError, assignmentsError, botsError]);

  function handleLogin(accessToken: string) {
    localStorage.setItem("google_token", accessToken);
    setToken(accessToken);
  }

  function handleLogout() {
    localStorage.removeItem("google_token");
    setToken(null);
  }

  if (!token) {
    return <LoginButton onLogin={handleLogin} />;
  }

  const tabs: { key: Tab; label: string }[] = [
    { key: "sessions", label: "Call Sessions" },
    { key: "profiles", label: "Agent Profiles" },
    { key: "assignments", label: "Assignments" },
    { key: "bots", label: "Bot Calls" },
  ];

  return (
    <div className="min-h-screen bg-gray-50">
      <header className="bg-white shadow-sm">
        <div className="mx-auto flex max-w-7xl items-center justify-between px-6 py-4">
          <h1 className="text-xl font-semibold text-gray-800">
            Steve Assist Dashboard
          </h1>
          <button
            onClick={handleLogout}
            className="text-sm text-gray-500 hover:text-gray-700 cursor-pointer"
          >
            Sign out
          </button>
        </div>
        <div className="mx-auto max-w-7xl px-6">
          <nav className="flex gap-6 border-b border-gray-200">
            {tabs.map((t) => (
              <button
                key={t.key}
                onClick={() => switchTab(t.key)}
                className={`cursor-pointer border-b-2 pb-3 text-sm font-medium transition-colors ${
                  tab === t.key
                    ? "border-blue-600 text-blue-600"
                    : "border-transparent text-gray-500 hover:text-gray-700"
                }`}
              >
                {t.label}
              </button>
            ))}
          </nav>
        </div>
      </header>
      <CalendarStatusBanner
        status={calendarStatus}
        loading={calendarLoading}
        error={calendarError}
        onReauth={reauthCalendar}
        onRefresh={refreshCalendar}
      />
      <main className="mx-auto max-w-7xl px-6 py-8">
        {tab === "sessions" && (
          <SessionsTable sessions={sessions} loading={sessionsLoading} error={sessionsError} />
        )}

        {tab === "profiles" && !editingProfile && (
          <ProfilesList
            profiles={profiles}
            loading={profilesLoading}
            error={profilesError}
            onSelect={setEditingProfile}
          />
        )}
        {tab === "profiles" && editingProfile && (
          <ProfileEditor
            token={token}
            profileId={editingProfile}
            onBack={() => setEditingProfile(null)}
          />
        )}

        {tab === "assignments" && !editingAssignment && (
          <AssignmentsList
            assignments={assignments}
            loading={assignmentsLoading}
            error={assignmentsError}
            onSelect={(id) => setEditingAssignment(id)}
            onNew={() => setEditingAssignment("new")}
          />
        )}
        {tab === "assignments" && editingAssignment && (
          <AssignmentEditor
            token={token}
            assignmentId={editingAssignment === "new" ? null : editingAssignment}
            profiles={profiles}
            onBack={() => setEditingAssignment(null)}
          />
        )}

        {tab === "bots" && (
          <BotGroupsList
            groups={botGroups}
            loading={botsLoading}
            error={botsError}
          />
        )}
      </main>
    </div>
  );
}
