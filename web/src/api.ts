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
  skills_coverage: number | null;
  years_fit: number | null;
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
  listings?: { id: string; url: string; source: string; is_canonical: boolean }[];
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
  years_have?: number | null;
  years_needed?: number | null;
  required?: boolean;
};

export type MatchSummary = {
  id: string;
  profile_id: string;
  overall: number;
  required_coverage: number | null;
  preferred_coverage: number | null;
  skills_coverage: number | null;
  years_fit: number | null;
  seniority_fit: number | null;
  comp_fit: number | null;
  location_fit: number | null;
  blocker_count: number;
  is_stale: boolean;
  computed_at: string;
  verdicts: RequirementVerdict[];
};

export type ExtractionConflict = {
  id: string;
  job_id: string;
  field: string;
  value_a: string | null;
  provenance_a: string | null;
  listing_a_id: string | null;
  value_b: string | null;
  provenance_b: string | null;
  listing_b_id: string | null;
  resolved_value: string | null;
  resolution: string;
  created_at: string;
};

export type DuplicateCandidate = {
  job_id: string;
  title: string;
  company_name: string;
  title_jaccard: number;
  description_cosine: number;
  strength: string;
  requirement_count: number;
  listings: { id: string; url: string; source: string; is_canonical: boolean }[];
  keep_this: boolean;
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

export type Me = {
  auth_mode: string;
  authenticated: boolean;
  name: string | null;
  scopes: string[];
};

export type ProfileSkill = {
  slug: string;
  years: number | null;
  last_used_year: number | null;
  is_primary: boolean;
  evidence_count: number;
};

export type Accomplishment = {
  id: string;
  text: string;
  strength: number;
  verified: boolean;
  skills: string[];
};

export type Experience = {
  id: string;
  kind: string;
  org: string;
  title: string | null;
  start_date: string | null;
  end_date: string | null;
  is_current: boolean;
  accomplishments: Accomplishment[];
};

export type Profile = {
  id: string;
  name: string;
  full_name: string | null;
  headline: string | null;
  location: string | null;
  summary_md: string | null;
  target_titles: string[];
  target_comp_min_cents: number | null;
  target_locations: string[];
  accepts_remote: boolean;
  years_experience: number | null;
  revision: number;
  skills: ProfileSkill[];
  experience: Experience[];
};

export type ImportReport = {
  profile_id: string;
  items: number;
  accomplishments: number;
  skills: number;
};

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(path, {
    credentials: "include",
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
  mergeJob: (from: string, into: string) =>
    request<{ from_id: string; into_id: string; listings_moved: number; requirements_added: number }>(
      `/api/v1/jobs/${from}/merge`,
      { method: "POST", body: JSON.stringify({ into_job_id: into }) },
    ),
  splitJob: (from: string, listingId: string) =>
    request<{ from_id: string; new_id: string; listing_id: string }>(
      `/api/v1/jobs/${from}/split`,
      { method: "POST", body: JSON.stringify({ listing_id: listingId }) },
    ),
  duplicates: (id: string) =>
    request<DuplicateCandidate[]>(`/api/v1/jobs/${id}/duplicates`),
  conflicts: (id: string) =>
    request<ExtractionConflict[]>(`/api/v1/jobs/${id}/conflicts`),
  resolveConflict: (jobId: string, conflictId: string, choice: "a" | "b" | "keep") =>
    request<ExtractionConflict>(`/api/v1/jobs/${jobId}/conflicts/${conflictId}/resolve`, {
      method: "POST",
      body: JSON.stringify({ choice }),
    }),
  task: (id: string) => request<TaskView>(`/api/v1/tasks/${id}`),
  ingestUrl: (url: string) =>
    request<Accepted>("/api/v1/ingest/url", { method: "POST", body: JSON.stringify({ url }) }),
  ingestPaste: (text: string, url?: string) =>
    request<Accepted>("/api/v1/ingest/paste", {
      method: "POST",
      body: JSON.stringify({ text, url }),
    }),
  meta: () => request<{ version: string; llm_provider: string }>("/api/v1/meta"),
  me: () => request<Me>("/api/v1/auth/me"),
  login: (username: string, password: string) =>
    request<Me>("/api/v1/auth/login", {
      method: "POST",
      body: JSON.stringify({ username, password }),
    }),
  logout: () =>
    fetch("/api/v1/auth/logout", { method: "POST", credentials: "include" }).then((res) => {
      if (!res.ok && res.status !== 204) throw new Error(res.statusText);
    }),
  profile: () => request<Profile>("/api/v1/profiles/default"),
  importResume: (text: string) =>
    request<ImportReport>("/api/v1/profiles/default/import-resume", {
      method: "POST",
      body: JSON.stringify({ text }),
    }),
};
