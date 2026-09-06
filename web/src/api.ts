// Wire types. The OpenAPI contract lives in `./api/generated.ts` (ADR-0009);
// this wrapper stays hand-written so error envelopes stay under our control.
export type JobListRow = {
  id: string;
  title: string;
  company_name: string;
  company_slug: string;
  status: string;
  work_mode: string;
  seniority: string;
  salary_min_cents: number | null;
  salary_max_cents: number | null;
  salary_currency: string | null;
  salary_period: string;
  posted_at: string | null;
  primary_location: string | null;
  extraction_partial: boolean;
  match_overall: number | null;
  updated_at: string;
};

export type RequirementRow = {
  id: string;
  text: string;
  kind: string;
  necessity: string;
  min_years: number | null;
  is_blocker: boolean;
};

export type JobDetail = {
  id: string;
  title: string;
  company_name: string;
  status: string;
  work_mode: string;
  seniority: string;
  employment_type: string;
  salary_raw: string | null;
  posted_at: string | null;
  apply_url: string | null;
  description_md: string;
  file_path: string | null;
  extraction_partial: boolean;
  locations: string[];
  requirements: RequirementRow[];
  user_rating: number | null;
  user_notes_md: string | null;
  is_archived: boolean;
  provenance: { field: string; provenance: string; confidence: number }[];
  updated_at: string;
};

export type RequirementVerdict = {
  requirement_id: string;
  status: string;
  score: number;
  rationale: string;
};

export type MatchSummary = {
  id: string;
  profile_id: string;
  overall: number;
  required_coverage: number | null;
  preferred_coverage: number | null;
  seniority_fit: number | null;
  comp_fit: number | null;
  location_fit: number | null;
  blocker_count: number;
  is_stale: boolean;
  computed_at: string;
  verdicts: RequirementVerdict[];
};

export type JobPatch = {
  title?: string;
  user_rating?: number;
  user_notes_md?: string;
  is_archived?: boolean;
  status?: string;
};

export type Page<T> = { items: T[]; next_cursor: string | null };

export type Accepted = {
  task_id: string;
  listing_id?: string;
  capture_id?: string;
};

export type TaskView = {
  id: string;
  kind: string;
  status: string;
  attempts: number;
  last_error: string | null;
  progress_message: string | null;
};

export type ApiError = { error: { code: string; message: string } };

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(path, {
    ...init,
    headers: { "content-type": "application/json", ...(init?.headers ?? {}) },
  });
  if (!res.ok) {
    let message = res.statusText;
    try {
      const body = (await res.json()) as ApiError;
      message = body.error?.message ?? message;
    } catch {
      /* keep status text */
    }
    throw new Error(message);
  }
  return (await res.json()) as T;
}

export const api = {
  jobs: (q?: string) =>
    request<Page<JobListRow>>(`/api/v1/jobs${q ? `?q=${encodeURIComponent(q)}` : ""}`),
  job: (id: string) => request<JobDetail>(`/api/v1/jobs/${id}`),
  jobMatch: (id: string) => request<MatchSummary>(`/api/v1/jobs/${id}/match`),
  patchJob: (id: string, body: JobPatch) =>
    request<JobDetail>(`/api/v1/jobs/${id}`, { method: "PATCH", body: JSON.stringify(body) }),
  task: (id: string) => request<TaskView>(`/api/v1/tasks/${id}`),
  ingestUrl: (url: string) =>
    request<Accepted>("/api/v1/ingest/url", { method: "POST", body: JSON.stringify({ url }) }),
  ingestPaste: (text: string, url?: string) =>
    request<Accepted>("/api/v1/ingest/paste", {
      method: "POST",
      body: JSON.stringify({ text, url }),
    }),
  meta: () => request<{ version: string; llm_provider: string }>("/api/v1/meta"),
  me: () =>
    request<{ auth_mode: string; authenticated: boolean; name: string | null; scopes: string[] }>(
      "/api/v1/auth/me",
    ),
};
