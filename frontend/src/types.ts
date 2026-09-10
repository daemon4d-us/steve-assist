export interface SessionSummary {
  call_sid: string;
  caller_phone: string;
  caller_name: string | null;
  direction: string;
  started_at: string;
  duration_seconds: number | null;
  summary: string | null;
}

export interface ProfileListItem {
  id: string;
  model: string;
  max_tokens: number;
}

export interface AgentProfile {
  base_prompt: string;
  new_caller_prompt: string;
  returning_caller_prompt: string;
  memory_prompt: string;
  outbound_prompt: string;
  model: string;
  max_tokens: number;
  timezone: string;
  working_hours_start: number;
  working_hours_end: number;
  working_days: number[];
  calendar_prompt: string;
  voice_id: string | null;
}

export interface AssignmentContact {
  phone: string;
  name: string | null;
  status: string;
  attempts: number;
  last_attempt_at: string | null;
  call_sid: string | null;
  error: string | null;
}

export interface Assignment {
  id: string;
  objective: string;
  profile_id: string;
  scheduled_at: string;
  status: string;
  created_at: string;
  contacts: AssignmentContact[];
}

export interface CreateAssignmentRequest {
  objective: string;
  profile_id: string;
  scheduled_at: string;
  contacts: { phone: string; name?: string }[];
}

export interface CalendarAccountStatus {
  label: string;
  account_email: string | null;
  valid: boolean;
  error: string | null;
  expires_at: string;
  calendar_count: number;
}

export interface CalendarConfigStatus {
  configured: boolean;
  accounts: CalendarAccountStatus[];
}

export interface BotCallGroup {
  id: string;
  name: string;
  phone_numbers: string[];
  latest_summary: string | null;
  latest_call_sid: string | null;
  latest_call_at: string;
  call_count: number;
  created_at: string;
}
