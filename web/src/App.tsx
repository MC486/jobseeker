import { FormEvent, useEffect, useMemo, useState } from "react";
import {
  keepPreviousData,
  useMutation,
  useQueries,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import {
  Link,
  Outlet,
  useNavigate,
  useParams,
  useRouterState,
  useSearch,
} from "@tanstack/react-router";
import {
  api,
  DuplicateCandidate,
  ExtractionConflict,
  JobDetail,
  JobListRow,
  MatchSummary,
  Me,
} from "./api";
import { MAX_COMPARE, parseCompareIds } from "./compare";
import { fetchers, keys, subscribeQueryEvents } from "./query";

export type JobSort = "overall" | "skills" | "years";

export function RootLayout() {
  const me = useQuery({ queryKey: keys.me, queryFn: fetchers.me, staleTime: 60_000 });
  const wide = useRouterState({
    select: (s) => s.location.pathname === "/jobs/compare",
  });
  useEffect(() => subscribeQueryEvents(), []);
  const shell = wide ? "max-w-6xl" : "max-w-4xl";

  return (
    <div className="min-h-screen">
      <header className="border-b border-zinc-800">
        <div className={`mx-auto flex ${shell} items-baseline justify-between px-5 py-4`}>
          <Link to="/" className="text-sm font-semibold tracking-wide">
            jobseeker
          </Link>
          <nav className="flex gap-4 text-sm text-zinc-400">
            <Link to="/jobs" className="hover:text-zinc-100">
              Jobs
            </Link>
            <Link to="/profile" className="hover:text-zinc-100">
              Profile
            </Link>
            <a href="/openapi.json" className="hover:text-zinc-100">
              OpenAPI
            </a>
            {me.data && <span title="auth.mode">auth:{me.data.auth_mode}</span>}
            {me.data?.auth_mode === "password" && me.data.authenticated && (
              <LogoutButton />
            )}
          </nav>
        </div>
      </header>
      <main className={`mx-auto ${shell} px-5 py-8`}>
        {me.data?.auth_mode === "password" && !me.data.authenticated ? (
          <Login />
        ) : (
          <Outlet />
        )}
      </main>
    </div>
  );
}

function LogoutButton() {
  const qc = useQueryClient();
  return (
    <button
      className="hover:text-zinc-100"
      onClick={() => {
        api.logout().finally(() => {
          qc.invalidateQueries({ queryKey: keys.me });
        });
      }}
    >
      Log out
    </button>
  );
}

function Login() {
  const qc = useQueryClient();
  const [username, setUsername] = useState("owner");
  const [password, setPassword] = useState("");
  const login = useMutation({
    mutationFn: () => api.login(username, password),
    onSuccess: (me: Me) => {
      qc.setQueryData(keys.me, me);
    },
  });

  function onSubmit(e: FormEvent) {
    e.preventDefault();
    login.mutate();
  }

  return (
    <form onSubmit={onSubmit} className="mx-auto max-w-sm space-y-4">
      <div>
        <h1 className="text-2xl font-semibold">Sign in</h1>
        <p className="mt-2 text-sm text-zinc-400">
          This instance uses a password. Create one with{" "}
          <code className="text-zinc-200">jobseeker user set-password</code>.
        </p>
      </div>
      <label className="block text-sm text-zinc-400">
        Username
        <input
          className="mt-1 w-full rounded-lg border border-zinc-700 bg-zinc-900 px-3 py-2 text-zinc-100"
          value={username}
          onChange={(e) => setUsername(e.target.value)}
          autoComplete="username"
          required
        />
      </label>
      <label className="block text-sm text-zinc-400">
        Password
        <input
          type="password"
          className="mt-1 w-full rounded-lg border border-zinc-700 bg-zinc-900 px-3 py-2 text-zinc-100"
          value={password}
          onChange={(e) => setPassword(e.target.value)}
          autoComplete="current-password"
          required
        />
      </label>
      {login.isError && (
        <p className="text-sm text-red-300">
          {login.error instanceof Error ? login.error.message : "login failed"}
        </p>
      )}
      <button
        disabled={login.isPending}
        className="rounded-lg bg-indigo-400 px-4 py-2 text-sm font-semibold text-zinc-950 disabled:opacity-50"
      >
        Sign in
      </button>
    </form>
  );
}

export function Home() {
  const navigate = useNavigate();
  const [url, setUrl] = useState("");
  const [paste, setPaste] = useState("");
  const [message, setMessage] = useState<string | null>(null);
  const ingestUrl = useMutation({
    mutationFn: () => api.ingestUrl(url),
    onSuccess: (accepted) => {
      setMessage(`Queued ${accepted.task_id}`);
      setUrl("");
      void navigate({ to: "/tasks/$taskId", params: { taskId: accepted.task_id } });
    },
    onError: (err) => {
      setMessage(err instanceof Error ? err.message : "failed");
    },
  });
  const ingestPaste = useMutation({
    mutationFn: () => api.ingestPaste(paste),
    onSuccess: (accepted) => {
      setMessage(`Queued ${accepted.task_id}`);
      setPaste("");
      void navigate({ to: "/tasks/$taskId", params: { taskId: accepted.task_id } });
    },
    onError: (err) => {
      setMessage(err instanceof Error ? err.message : "failed");
    },
  });
  const busy = ingestUrl.isPending || ingestPaste.isPending;

  return (
    <div className="space-y-8">
      <div>
        <h1 className="text-2xl font-semibold">Ingest a posting</h1>
        <p className="mt-2 text-zinc-400">
          Public ATS URLs are fetched here. LinkedIn and Indeed need the browser extension —
          this server will not log in for you.
        </p>
      </div>
      <form
        onSubmit={(e) => {
          e.preventDefault();
          ingestUrl.mutate();
        }}
        className="space-y-3"
      >
        <label className="block text-sm text-zinc-400">
          Public job URL
          <input
            className="mt-1 w-full rounded-lg border border-zinc-700 bg-zinc-900 px-3 py-2 text-zinc-100"
            value={url}
            onChange={(e) => setUrl(e.target.value)}
            placeholder="https://boards.greenhouse.io/…/jobs/123"
            required
          />
        </label>
        <button
          disabled={busy}
          className="rounded-lg bg-indigo-400 px-4 py-2 text-sm font-semibold text-zinc-950 disabled:opacity-50"
        >
          Ingest URL
        </button>
      </form>
      <form
        onSubmit={(e) => {
          e.preventDefault();
          ingestPaste.mutate();
        }}
        className="space-y-3"
      >
        <label className="block text-sm text-zinc-400">
          Or paste HTML / text
          <textarea
            className="mt-1 min-h-36 w-full rounded-lg border border-zinc-700 bg-zinc-900 px-3 py-2 text-zinc-100"
            value={paste}
            onChange={(e) => setPaste(e.target.value)}
            required
          />
        </label>
        <button
          disabled={busy}
          className="rounded-lg bg-zinc-100 px-4 py-2 text-sm font-semibold text-zinc-950 disabled:opacity-50"
        >
          Ingest paste
        </button>
      </form>
      {message && <p className="text-sm text-zinc-300">{message}</p>}
    </div>
  );
}

function scoreOf(row: JobListRow, sort: JobSort): number {
  const value =
    sort === "skills"
      ? row.skills_coverage
      : sort === "years"
        ? row.years_fit
        : row.match_overall;
  return value ?? -1;
}

export function JobList() {
  const navigate = useNavigate({ from: "/jobs" });
  const search = useSearch({ from: "/jobs" });
  const sort: JobSort = search.sort ?? "overall";
  const [draft, setDraft] = useState(search.q ?? "");

  useEffect(() => {
    setDraft(search.q ?? "");
  }, [search.q]);

  useEffect(() => {
    const next = draft.trim() || undefined;
    if (next === search.q) return;
    const timer = window.setTimeout(() => {
      void navigate({
        search: (prev) => ({ ...prev, q: next }),
        replace: true,
      });
    }, 200);
    return () => window.clearTimeout(timer);
  }, [draft, navigate, search.q]);

  const jobs = useQuery({
    queryKey: keys.jobs(search.q),
    queryFn: () => fetchers.jobs(search.q),
    placeholderData: keepPreviousData,
  });

  const rows = useMemo(() => {
    const items = jobs.data?.items ?? [];
    return [...items].sort((a, b) => scoreOf(b, sort) - scoreOf(a, sort));
  }, [jobs.data, sort]);
  const [picked, setPicked] = useState<string[]>([]);

  function togglePick(id: string) {
    setPicked((prev) => {
      if (prev.includes(id)) return prev.filter((item) => item !== id);
      if (prev.length >= MAX_COMPARE) return prev;
      return [...prev, id];
    });
  }

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-end justify-between gap-3">
        <h1 className="text-2xl font-semibold">Jobs</h1>
        {picked.length >= 2 ? (
          <Link
            to="/jobs/compare"
            search={{ ids: picked.join(",") }}
            className="rounded-lg bg-indigo-400 px-3 py-1.5 text-sm font-semibold text-zinc-950"
          >
            Compare {picked.length}
          </Link>
        ) : (
          <span className="text-xs text-zinc-600">Select 2–4 to compare</span>
        )}
      </div>
      <input
        className="w-full rounded-lg border border-zinc-700 bg-zinc-900 px-3 py-2"
        placeholder="Search title, company, requirements…"
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
      />
      <div className="flex flex-wrap gap-3 text-xs">
        <span className="text-zinc-500">Sort</span>
        <SortChip current={sort} value="overall" label="Overall" />
        <SortChip current={sort} value="skills" label="Skills" />
        <SortChip current={sort} value="years" label="Years" />
        <span className="text-zinc-600">
          client-side on this page · does not change overall
        </span>
      </div>
      {jobs.isError && (
        <p className="text-sm text-red-300">
          {jobs.error instanceof Error ? jobs.error.message : "failed"}
        </p>
      )}
      <ul className="divide-y divide-zinc-800">
        {rows.map((row) => {
          const checked = picked.includes(row.id);
          const locked = !checked && picked.length >= MAX_COMPARE;
          return (
            <li key={row.id} className="flex items-start gap-3 py-3">
              <input
                type="checkbox"
                className="mt-1.5 accent-indigo-400"
                checked={checked}
                disabled={locked}
                aria-label={`Select ${row.title} at ${row.company_name} for compare`}
                onChange={() => togglePick(row.id)}
              />
              <Link to="/jobs/$jobId" params={{ jobId: row.id }} className="min-w-0 flex-1 text-left">
                <div className="font-medium">{row.title}</div>
                <div className="text-sm text-zinc-400">
                  {row.company_name} · {row.work_mode} · {row.status}
                  {row.primary_location ? ` · ${row.primary_location}` : ""}
                </div>
                {row.match_overall != null ? (
                  <div className="mt-1 flex flex-wrap gap-x-3 text-xs text-zinc-400">
                    <span className={sort === "overall" ? "text-zinc-100" : "text-zinc-200"}>
                      {pct(row.match_overall)} match
                    </span>
                    {row.skills_coverage != null ? (
                      <span
                        className={sort === "skills" ? "text-zinc-100" : undefined}
                        title="Required bars with tenure stripped. Does not change overall."
                      >
                        skills {pct(row.skills_coverage)}
                      </span>
                    ) : null}
                    {row.years_fit != null ? (
                      <span
                        className={sort === "years" ? "text-zinc-100" : undefined}
                        title="Required year-count asks. Recruiters overfit this; postings often mean a wishlist."
                      >
                        years {pct(row.years_fit)}
                      </span>
                    ) : null}
                  </div>
                ) : null}
              </Link>
            </li>
          );
        })}
      </ul>
      {rows.length === 0 && !jobs.isError && !jobs.isPending && (
        <p className="text-zinc-500">No jobs yet.</p>
      )}
    </div>
  );
}

function SortChip({
  current,
  value,
  label,
}: {
  current: JobSort;
  value: JobSort;
  label: string;
}) {
  const active = current === value;
  return (
    <Link
      to="/jobs"
      search={(prev) => ({
        ...prev,
        sort: value === "overall" ? undefined : value,
      })}
      replace
      className={
        active
          ? "rounded-md bg-zinc-700 px-2 py-0.5 text-zinc-50 ring-1 ring-zinc-400"
          : "rounded-md px-2 py-0.5 text-zinc-400 hover:text-zinc-200"
      }
    >
      {label}
    </Link>
  );
}

const COMPARE_SCORES: {
  key: "overall" | "skills_coverage" | "years_fit" | "required_coverage" | "preferred_coverage" | "seniority_fit" | "comp_fit" | "location_fit";
  label: string;
  hint?: string;
}[] = [
  { key: "overall", label: "Overall", hint: "unchanged weighted score" },
  { key: "skills_coverage", label: "Skills", hint: "tenure stripped" },
  { key: "years_fit", label: "Years", hint: "year-count asks" },
  { key: "required_coverage", label: "Required", hint: "feeds overall" },
  { key: "preferred_coverage", label: "Preferred" },
  { key: "seniority_fit", label: "Seniority" },
  { key: "comp_fit", label: "Comp" },
  { key: "location_fit", label: "Location" },
];

function scoreValue(
  match: MatchSummary | undefined,
  key: (typeof COMPARE_SCORES)[number]["key"],
): number | null {
  if (!match) return null;
  if (key === "overall") return match.overall;
  return match[key] ?? null;
}

function bestIndexes(values: Array<number | null>): Set<number> {
  let top = -Infinity;
  const winners = new Set<number>();
  values.forEach((value, i) => {
    if (value == null) return;
    if (value > top) {
      top = value;
      winners.clear();
      winners.add(i);
    } else if (value === top) {
      winners.add(i);
    }
  });
  return winners.size === values.filter((v) => v != null).length ? new Set() : winners;
}

function sameCompany(a: JobDetail, b: JobDetail): boolean {
  const left = a.company_name.trim().toLowerCase();
  const right = b.company_name.trim().toLowerCase();
  return left.length > 0 && left === right;
}

function listingHint(job: JobDetail): string {
  const sources = [...new Set((job.listings ?? []).map((l) => l.source))];
  const src = sources.length ? sources.join("+") : "posting";
  return `${src} · ${job.requirements.length} reqs`;
}

const ATS_SOURCES = new Set([
  "workday",
  "greenhouse",
  "lever",
  "ashby",
  "company_site",
]);

function keeperRank(job: JobDetail): number {
  const ats = (job.listings ?? []).some((l) => ATS_SOURCES.has(l.source))
    ? 1
    : 0;
  return job.requirements.length * 10 + ats;
}

export function ComparePage() {
  const navigate = useNavigate({ from: "/jobs/compare" });
  const search = useSearch({ from: "/jobs/compare" });
  const qc = useQueryClient();
  const ids = parseCompareIds(search.ids);
  const [merging, setMerging] = useState<string | null>(null);
  const [mergeError, setMergeError] = useState<string | null>(null);
  const details = useQueries({
    queries: ids.map((id) => ({
      queryKey: keys.job(id),
      queryFn: () => fetchers.job(id),
    })),
  });
  const matches = useQueries({
    queries: ids.map((id) => ({
      queryKey: keys.match(id),
      queryFn: () => fetchers.match(id),
      retry: false,
    })),
  });

  async function mergeInto(fromId: string, intoId: string) {
    setMerging(`${fromId}-${intoId}`);
    setMergeError(null);
    try {
      await api.mergeJob(fromId, intoId);
      await qc.invalidateQueries({ queryKey: ["jobs"] });
      await qc.invalidateQueries({ queryKey: keys.job(intoId) });
      await navigate({ to: "/jobs/$jobId", params: { jobId: intoId } });
    } catch (err) {
      setMergeError(err instanceof Error ? err.message : String(err));
    } finally {
      setMerging(null);
    }
  }

  function drop(id: string) {
    const next = ids.filter((item) => item !== id);
    void navigate({
      search: { ids: next.length ? next.join(",") : undefined },
      replace: true,
    });
  }

  if (ids.length < 2) {
    return (
      <div className="space-y-3">
        <h1 className="text-2xl font-semibold">Compare</h1>
        <p className="text-sm text-zinc-400">
          Select 2–4 jobs on the list. Overall is unchanged — this page only lines the
          same numbers up.
        </p>
        <Link to="/jobs" className="text-sm text-indigo-300">
          Back to jobs
        </Link>
      </div>
    );
  }

  const loading = details.some((q) => q.isPending);
  const columns = ids.map((id, i) => ({
    id,
    job: details[i]?.data,
    match: matches[i]?.data,
    error: details[i]?.error,
  }));
  const loaded = columns
    .map((col) => col.job)
    .filter((job): job is JobDetail => job != null);
  const mergePairs = loaded.flatMap((from) =>
    loaded
      .filter((into) => into.id !== from.id && sameCompany(from, into))
      .map((into) => ({
        from,
        into,
        recommended: keeperRank(into) >= keeperRank(from),
      })),
  );
  mergePairs.sort((a, b) => Number(b.recommended) - Number(a.recommended));

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-end justify-between gap-3">
        <div>
          <h1 className="text-2xl font-semibold">Compare</h1>
          <p className="mt-1 text-xs text-zinc-500">
            Highest value in a score row is highlighted. Skills and Years are diagnostic
            and do not change overall.
          </p>
        </div>
        <Link to="/jobs" className="text-sm text-zinc-400 hover:text-zinc-100">
          Back to jobs
        </Link>
      </div>
      {mergePairs.length > 0 && (
        <div className="space-y-2 rounded-xl border border-zinc-800 bg-zinc-900/40 p-3">
          <p className="text-xs text-zinc-500">
            Same company — merge the thinner capture into the fuller one. The
            keeper keeps its title and description; the donor is tombstoned.
            Prefer the ATS posting when one side is an aggregator.
          </p>
          <div className="flex flex-wrap gap-2">
            {mergePairs.map(({ from, into, recommended }) => {
              const key = `${from.id}-${into.id}`;
              return (
                <button
                  key={key}
                  type="button"
                  disabled={merging != null}
                  onClick={() => void mergeInto(from.id, into.id)}
                  className={
                    recommended
                      ? "rounded-lg bg-indigo-400 px-3 py-1.5 text-sm font-semibold text-zinc-950 disabled:opacity-50"
                      : "rounded-lg border border-zinc-700 px-3 py-1.5 text-sm text-zinc-300 disabled:opacity-50"
                  }
                >
                  {merging === key
                    ? "Merging…"
                    : `Merge ${listingHint(from)} into ${listingHint(into)}`}
                </button>
              );
            })}
          </div>
        </div>
      )}
      {mergeError && <p className="text-sm text-red-300">{mergeError}</p>}
      {loading && <p className="text-sm text-zinc-500">Loading…</p>}
      <div className="overflow-x-auto">
        <table className="w-full min-w-[40rem] border-collapse text-sm">
          <thead>
            <tr className="border-b border-zinc-800 align-top">
              <th className="w-28 py-2 text-left text-xs font-normal uppercase tracking-wide text-zinc-500">
                Field
              </th>
              {columns.map((col) => (
                <th key={col.id} className="px-3 py-2 text-left font-medium">
                  {col.job ? (
                    <>
                      <Link
                        to="/jobs/$jobId"
                        params={{ jobId: col.id }}
                        className="text-zinc-100 hover:text-indigo-300"
                      >
                        {col.job.title}
                      </Link>
                      <div className="text-xs font-normal text-zinc-400">
                        {col.job.company_name}
                      </div>
                      <button
                        type="button"
                        className="mt-1 text-xs font-normal text-zinc-500 hover:text-zinc-200"
                        onClick={() => drop(col.id)}
                      >
                        Remove
                      </button>
                    </>
                  ) : col.error ? (
                    <span className="text-red-300">missing</span>
                  ) : (
                    <span className="text-zinc-500">…</span>
                  )}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            <tr className="border-b border-zinc-800/80">
              <th className="py-2 text-left text-xs font-normal text-zinc-500">Sources</th>
              {columns.map((col) => (
                <td key={col.id} className="px-3 py-2 text-zinc-300">
                  {(col.job?.listings ?? []).length
                    ? (col.job?.listings ?? [])
                        .map((l) =>
                          l.is_canonical ? `${l.source} (canonical)` : l.source,
                        )
                        .join(" · ")
                    : "—"}
                </td>
              ))}
            </tr>
            <tr className="border-b border-zinc-800/80">
              <th className="py-2 text-left text-xs font-normal text-zinc-500">Location</th>
              {columns.map((col) => (
                <td key={col.id} className="px-3 py-2 text-zinc-300">
                  {col.job?.locations.join(" · ") || "—"}
                </td>
              ))}
            </tr>
            <tr className="border-b border-zinc-800/80">
              <th className="py-2 text-left text-xs font-normal text-zinc-500">Mode</th>
              {columns.map((col) => (
                <td key={col.id} className="px-3 py-2 text-zinc-300">
                  {col.job ? `${col.job.work_mode} · ${col.job.seniority}` : "—"}
                </td>
              ))}
            </tr>
            <tr className="border-b border-zinc-800/80">
              <th className="py-2 text-left text-xs font-normal text-zinc-500">Comp</th>
              {columns.map((col) => (
                <td key={col.id} className="px-3 py-2 text-zinc-300">
                  {col.job?.salary_raw || "—"}
                </td>
              ))}
            </tr>
            {COMPARE_SCORES.map((row) => {
              const values = columns.map((col) => scoreValue(col.match, row.key));
              const winners = bestIndexes(values);
              return (
                <tr key={row.key} className="border-b border-zinc-800/80">
                  <th className="py-2 text-left text-xs font-normal text-zinc-500">
                    <span>{row.label}</span>
                    {row.hint ? (
                      <span className="mt-0.5 block font-normal normal-case tracking-normal text-zinc-600">
                        {row.hint}
                      </span>
                    ) : null}
                  </th>
                  {values.map((value, i) => (
                    <td
                      key={columns[i].id}
                      className={`px-3 py-2 ${winners.has(i) ? "font-medium text-emerald-300" : "text-zinc-200"}`}
                    >
                      {pct(value)}
                    </td>
                  ))}
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
      <section className="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
        {columns.map((col) => {
          const reqs = col.job?.requirements ?? [];
          const verdicts = new Map(
            (col.match?.verdicts ?? []).map((v) => [v.requirement_id, v]),
          );
          const shown = reqs.slice(0, 12);
          return (
            <div key={col.id} className="rounded-xl border border-zinc-800 p-3">
              <h2 className="text-sm font-medium text-zinc-200">
                {col.job?.title ?? "Requirements"}
              </h2>
              <ul className="mt-2 space-y-2 text-xs text-zinc-400">
                {shown.map((r) => {
                  const verdict = verdicts.get(r.id);
                  return (
                    <li key={r.id}>
                      <span className="text-zinc-500">{r.necessity}</span> {r.text}
                      {verdict ? (
                        <span className="ml-1 text-zinc-300">
                          · {verdict.status} ({Math.round(verdict.score * 100)}%)
                          {verdict.years_needed != null
                            ? ` · ${fmtYears(verdict.years_have)} / ${fmtYears(verdict.years_needed)} yr`
                            : ""}
                        </span>
                      ) : null}
                    </li>
                  );
                })}
              </ul>
              {reqs.length > shown.length ? (
                <Link
                  to="/jobs/$jobId"
                  params={{ jobId: col.id }}
                  className="mt-2 inline-block text-xs text-indigo-300"
                >
                  {reqs.length - shown.length} more
                </Link>
              ) : null}
            </div>
          );
        })}
      </section>
    </div>
  );
}

function DuplicateRow({
  currentId,
  candidate,
  onMerged,
}: {
  currentId: string;
  candidate: DuplicateCandidate;
  onMerged: (intoId: string) => void;
}) {
  const qc = useQueryClient();
  const [error, setError] = useState<string | null>(null);
  const merge = useMutation({
    mutationFn: ({ from, into }: { from: string; into: string }) =>
      api.mergeJob(from, into),
    onSuccess: async (_report, vars) => {
      await qc.invalidateQueries({ queryKey: ["jobs"] });
      await qc.invalidateQueries({ queryKey: ["duplicates"] });
      onMerged(vars.into);
    },
    onError: (err) => {
      setError(err instanceof Error ? err.message : String(err));
    },
  });
  const sources = [...new Set(candidate.listings.map((l) => l.source))].join("+") || "posting";
  const mergeThisIntoThem = { from: currentId, into: candidate.job_id };
  const mergeThemIntoThis = { from: candidate.job_id, into: currentId };
  const recommended = candidate.keep_this ? mergeThisIntoThem : mergeThemIntoThis;

  return (
    <div className="space-y-2 rounded-lg border border-zinc-800 bg-zinc-950/40 p-3">
      <div className="flex flex-wrap items-baseline justify-between gap-2">
        <Link
          to="/jobs/$jobId"
          params={{ jobId: candidate.job_id }}
          className="font-medium text-zinc-100 hover:text-indigo-300"
        >
          {candidate.title}
        </Link>
        <span className="text-xs uppercase tracking-wide text-zinc-500">
          {candidate.strength}
        </span>
      </div>
      <p className="text-xs text-zinc-500">
        {candidate.company_name} · {sources} · {candidate.requirement_count} reqs · title{" "}
        {Math.round(candidate.title_jaccard * 100)}%
        {candidate.description_cosine > 0
          ? ` · desc ${Math.round(candidate.description_cosine * 100)}%`
          : ""}
      </p>
      {error && <p className="text-sm text-red-300">{error}</p>}
      <div className="flex flex-wrap gap-2">
        <Link
          to="/jobs/compare"
          search={{ ids: `${currentId},${candidate.job_id}` }}
          className="rounded-lg border border-zinc-700 px-3 py-1.5 text-sm text-zinc-300"
        >
          Compare
        </Link>
        <button
          type="button"
          disabled={merge.isPending}
          onClick={() => merge.mutate(recommended)}
          className="rounded-lg bg-indigo-400 px-3 py-1.5 text-sm font-semibold text-zinc-950 disabled:opacity-50"
        >
          {recommended.from === currentId
            ? `Merge this into ${sources}`
            : `Merge ${sources} into this`}
        </button>
        <button
          type="button"
          disabled={merge.isPending}
          onClick={() =>
            merge.mutate(
              recommended.from === currentId ? mergeThemIntoThis : mergeThisIntoThem,
            )
          }
          className="rounded-lg border border-zinc-700 px-3 py-1.5 text-sm text-zinc-300 disabled:opacity-50"
        >
          Other direction
        </button>
      </div>
    </div>
  );
}

function ConflictRow({
  currentSalary,
  conflict,
}: {
  currentSalary: string | null;
  conflict: ExtractionConflict;
}) {
  const qc = useQueryClient();
  const [error, setError] = useState<string | null>(null);
  const resolve = useMutation({
    mutationFn: (choice: "a" | "b" | "keep") =>
      api.resolveConflict(conflict.job_id, conflict.id, choice),
    onSuccess: async () => {
      await qc.invalidateQueries({ queryKey: ["conflicts"] });
      await qc.invalidateQueries({ queryKey: keys.job(conflict.job_id) });
      await qc.invalidateQueries({ queryKey: ["jobs"] });
      await qc.invalidateQueries({ queryKey: keys.match(conflict.job_id) });
    },
    onError: (err) => {
      setError(err instanceof Error ? err.message : String(err));
    },
  });
  const labelA = conflict.provenance_a ?? "A";
  const labelB = conflict.provenance_b ?? "B";

  return (
    <div className="space-y-2 rounded-lg border border-zinc-800 bg-zinc-950/40 p-3">
      <p className="text-xs uppercase tracking-wide text-zinc-500">{conflict.field}</p>
      <p className="text-sm text-zinc-200">
        <span className="text-zinc-500">{labelA}:</span> {conflict.value_a ?? "—"}
      </p>
      <p className="text-sm text-zinc-200">
        <span className="text-zinc-500">{labelB}:</span> {conflict.value_b ?? "—"}
      </p>
      {currentSalary ? (
        <p className="text-xs text-zinc-500">Current: {currentSalary}</p>
      ) : null}
      {error && <p className="text-sm text-red-300">{error}</p>}
      <div className="flex flex-wrap gap-2">
        <button
          type="button"
          disabled={resolve.isPending}
          onClick={() => resolve.mutate("a")}
          className="rounded-lg bg-indigo-400 px-3 py-1.5 text-sm font-semibold text-zinc-950 disabled:opacity-50"
        >
          Use {labelA}
        </button>
        <button
          type="button"
          disabled={resolve.isPending}
          onClick={() => resolve.mutate("b")}
          className="rounded-lg border border-zinc-700 px-3 py-1.5 text-sm text-zinc-300 disabled:opacity-50"
        >
          Use {labelB}
        </button>
        <button
          type="button"
          disabled={resolve.isPending}
          onClick={() => resolve.mutate("keep")}
          className="rounded-lg border border-zinc-700 px-3 py-1.5 text-sm text-zinc-300 disabled:opacity-50"
        >
          Keep current
        </button>
      </div>
    </div>
  );
}

function provenanceDot(job: JobDetail, field: string) {
  const row = job.provenance.find((p) => p.field === field);
  if (!row) return null;
  const color =
    row.provenance === "manual"
      ? "bg-emerald-400"
      : row.confidence >= 0.85
        ? "bg-indigo-400"
        : row.confidence >= 0.6
          ? "bg-amber-400"
          : "bg-zinc-500";
  return (
    <span
      className={`ml-2 inline-block h-2 w-2 rounded-full ${color}`}
      title={`${row.field}: ${row.provenance} (${Math.round(row.confidence * 100)}%)`}
    />
  );
}

export function JobPage() {
  const { jobId } = useParams({ from: "/jobs/$jobId" });
  const navigate = useNavigate();
  const qc = useQueryClient();
  const [splitError, setSplitError] = useState<string | null>(null);
  const job = useQuery({ queryKey: keys.job(jobId), queryFn: () => fetchers.job(jobId) });
  const match = useQuery({
    queryKey: keys.match(jobId),
    queryFn: () => fetchers.match(jobId),
    retry: false,
  });
  const dups = useQuery({
    queryKey: keys.duplicates(jobId),
    queryFn: () => fetchers.duplicates(jobId),
  });
  const conflicts = useQuery({
    queryKey: keys.conflicts(jobId),
    queryFn: () => fetchers.conflicts(jobId),
  });
  const archive = useMutation({
    mutationFn: (next: boolean) => api.patchJob(jobId, { is_archived: next }),
    onSuccess: (detail) => {
      qc.setQueryData(keys.job(jobId), detail);
      qc.invalidateQueries({ queryKey: ["jobs"] });
    },
  });
  const split = useMutation({
    mutationFn: (listingId: string) => api.splitJob(jobId, listingId),
    onSuccess: async (report) => {
      await qc.invalidateQueries({ queryKey: ["jobs"] });
      await qc.invalidateQueries({ queryKey: keys.job(jobId) });
      await navigate({ to: "/jobs/$jobId", params: { jobId: report.new_id } });
    },
    onError: (err) => {
      setSplitError(err instanceof Error ? err.message : String(err));
    },
  });

  if (job.isError) {
    return (
      <p className="text-red-300">
        {job.error instanceof Error ? job.error.message : "failed"}
      </p>
    );
  }
  if (!job.data) return <p className="text-zinc-500">Loading…</p>;
  const detail = job.data;
  const score = match.data ?? null;
  const verdictByReq = new Map((score?.verdicts ?? []).map((v) => [v.requirement_id, v]));
  return (
    <article className="space-y-6">
      <div>
        <p className="text-sm text-zinc-400">{detail.company_name}</p>
        <h1 className="text-2xl font-semibold">
          {detail.title}
          {provenanceDot(detail, "title")}
        </h1>
        <p className="mt-1 text-sm text-zinc-400">
          {detail.work_mode} · {detail.seniority} · {detail.employment_type}
          {detail.extraction_partial ? " · partial extraction" : ""}
          {detail.is_archived ? " · archived" : ""}
        </p>
        {detail.apply_url && (
          <a className="mt-2 inline-block text-sm text-indigo-300" href={detail.apply_url}>
            Apply
          </a>
        )}
        <div className="mt-3">
          <button
            disabled={archive.isPending}
            onClick={() => archive.mutate(!detail.is_archived)}
            className="rounded-lg border border-zinc-700 px-3 py-1 text-sm text-zinc-300 disabled:opacity-50"
          >
            {detail.is_archived ? "Unarchive" : "Archive"}
          </button>
        </div>
      </div>
      {(conflicts.data ?? []).some((c) => c.resolution === "unresolved") && (
        <section className="space-y-2 rounded-xl border border-sky-900/60 bg-sky-950/20 p-4">
          <h2 className="text-lg font-medium text-sky-100">Extraction conflicts</h2>
          <p className="text-xs text-zinc-500">
            Sources disagreed. Nothing is picked until you say so — the ATS wording is
            usually the one you want.
          </p>
          {(conflicts.data ?? [])
            .filter((c) => c.resolution === "unresolved")
            .map((c) => (
              <ConflictRow key={c.id} currentSalary={detail.salary_raw} conflict={c} />
            ))}
        </section>
      )}
      {(dups.data ?? []).length > 0 && (
        <section className="space-y-2 rounded-xl border border-amber-900/60 bg-amber-950/20 p-4">
          <h2 className="text-lg font-medium text-amber-100">Possible duplicates</h2>
          <p className="text-xs text-zinc-500">
            Same company, similar title. Nothing is merged until you say so.
          </p>
          {(dups.data ?? []).map((c) => (
            <DuplicateRow
              key={c.job_id}
              currentId={jobId}
              candidate={c}
              onMerged={(intoId) =>
                void navigate({ to: "/jobs/$jobId", params: { jobId: intoId } })
              }
            />
          ))}
        </section>
      )}
      {score && (
        <section className="rounded-xl border border-zinc-800 bg-zinc-900/60 p-4">
          <h2 className="text-lg font-medium">
            Match {Math.round(score.overall * 100)}%
            {score.is_stale ? " · stale" : ""}
          </h2>
          <p className="mt-1 text-xs text-zinc-500">
            Overall is unchanged. Skills ignore year shortfalls when the skill is on the
            bank; Years is the tenure bar recruiters overfit and postings often treat as a
            wishlist.
          </p>
          <dl className="mt-3 grid grid-cols-2 gap-2 text-sm sm:grid-cols-4">
            <ScoreCell
              label="Skills"
              value={score.skills_coverage}
              hint="required bars, tenure stripped"
            />
            <ScoreCell
              label="Years"
              value={score.years_fit}
              hint="required year-count asks"
            />
            <ScoreCell
              label="Required"
              value={score.required_coverage}
              hint="feeds overall (includes years)"
            />
            <ScoreCell label="Preferred" value={score.preferred_coverage} />
            <ScoreCell label="Seniority" value={score.seniority_fit} />
            <ScoreCell label="Comp" value={score.comp_fit} />
            <ScoreCell label="Location" value={score.location_fit} />
          </dl>
          {score.blocker_count > 0 ? (
            <p className="mt-2 text-sm text-amber-300">
              {score.blocker_count} blocker(s) — overall is capped
            </p>
          ) : null}
        </section>
      )}
      {detail.salary_raw && (
        <p>
          Comp: {detail.salary_raw}
          {provenanceDot(detail, "salary")}
        </p>
      )}
      {detail.locations.length > 0 && (
        <p className="text-zinc-300">Location: {detail.locations.join(" · ")}</p>
      )}
      {(detail.listings ?? []).length > 0 && (
        <section>
          <h2 className="mb-2 text-lg font-medium">Listings</h2>
          <p className="mb-2 text-xs text-zinc-500">
            {(detail.listings ?? []).length >= 2
              ? "Split off a listing to undo a merge. The original keeps its title and description."
              : "This job has one source URL."}
          </p>
          {splitError && <p className="mb-2 text-sm text-red-300">{splitError}</p>}
          <ul className="space-y-2 text-sm text-zinc-300">
            {(detail.listings ?? []).map((listing) => (
              <li key={listing.id} className="flex flex-wrap items-baseline gap-x-2">
                <span className="text-zinc-500">{listing.source}</span>
                {listing.is_canonical ? (
                  <span className="text-xs text-emerald-400">canonical</span>
                ) : null}
                <a className="break-all text-indigo-300" href={listing.url}>
                  {listing.url}
                </a>
                {(detail.listings ?? []).length >= 2 ? (
                  <button
                    type="button"
                    disabled={split.isPending}
                    onClick={() => split.mutate(listing.id)}
                    className="text-xs text-zinc-500 hover:text-zinc-200 disabled:opacity-50"
                  >
                    {split.isPending && split.variables === listing.id
                      ? "Splitting…"
                      : "Split off"}
                  </button>
                ) : null}
              </li>
            ))}
          </ul>
        </section>
      )}
      <section>
        <h2 className="mb-2 text-lg font-medium">Requirements</h2>
        <ul className="space-y-2 text-sm">
          {detail.requirements.map((r) => {
            const verdict = verdictByReq.get(r.id);
            return (
              <li key={r.id}>
                <div>
                  <span className="text-zinc-500">{r.necessity}</span> {r.text}
                  {r.is_blocker ? " · blocker" : ""}
                  {verdict ? (
                    <span className="ml-2 text-zinc-400">
                      · {verdict.status} ({Math.round(verdict.score * 100)}%)
                      {verdict.years_needed != null
                        ? ` · ${fmtYears(verdict.years_have)} / ${fmtYears(verdict.years_needed)} yr`
                        : ""}
                    </span>
                  ) : null}
                </div>
                {verdict?.rationale && (
                  <p className="text-zinc-500">{verdict.rationale}</p>
                )}
              </li>
            );
          })}
        </ul>
      </section>
      <section>
        <h2 className="mb-2 text-lg font-medium">Description</h2>
        <pre className="whitespace-pre-wrap text-sm text-zinc-300">{detail.description_md}</pre>
      </section>
    </article>
  );
}

function pct(value: number | null | undefined) {
  return value == null ? "—" : `${Math.round(value * 100)}%`;
}

function fmtYears(value: number | null | undefined) {
  if (value == null) return "0";
  return Number.isInteger(value) ? String(value) : value.toFixed(1);
}

function ScoreCell({
  label,
  value,
  hint,
}: {
  label: string;
  value: number | null | undefined;
  hint?: string;
}) {
  return (
    <div className="rounded-lg border border-zinc-800 px-3 py-2">
      <dt className="text-xs uppercase tracking-wide text-zinc-500">{label}</dt>
      <dd className="text-lg font-medium text-zinc-100">{pct(value)}</dd>
      {hint ? <p className="text-[11px] text-zinc-500">{hint}</p> : null}
    </div>
  );
}

export function TaskPage() {
  const { taskId } = useParams({ from: "/tasks/$taskId" });
  const task = useQuery({
    queryKey: keys.task(taskId),
    queryFn: () => fetchers.task(taskId),
  });
  const done = useMemo(
    () => task.data && ["done", "failed", "cancelled"].includes(task.data.status),
    [task.data],
  );

  if (task.isError) {
    return (
      <p className="text-red-300">
        {task.error instanceof Error ? task.error.message : "failed"}
      </p>
    );
  }
  if (!task.data) return <p className="text-zinc-500">Loading…</p>;
  return (
    <div className="space-y-4">
      <h1 className="text-2xl font-semibold">Task {task.data.kind}</h1>
      <p className="text-zinc-400">
        {task.data.status}
        {task.data.progress_message ? ` — ${task.data.progress_message}` : ""}
      </p>
      {task.data.last_error && <p className="text-red-300">{task.data.last_error}</p>}
      {done && (
        <Link to="/jobs" className="text-indigo-300">
          View jobs
        </Link>
      )}
    </div>
  );
}

export function ProfilePage() {
  const qc = useQueryClient();
  const [text, setText] = useState("");
  const profile = useQuery({ queryKey: keys.profile, queryFn: fetchers.profile });
  const importResume = useMutation({
    mutationFn: () => api.importResume(text),
    onSuccess: async () => {
      await qc.invalidateQueries({ queryKey: keys.profile });
      await qc.invalidateQueries({ queryKey: ["jobs"] });
      await qc.invalidateQueries({ queryKey: ["match"] });
    },
  });

  return (
    <div className="space-y-8">
      <div>
        <h1 className="text-2xl font-semibold">Experience bank</h1>
        <p className="mt-2 text-sm text-zinc-400">
          Paste a Markdown evidence bank. Import replaces the default profile and rescores every
          saved job. Nothing here is invented — only what the file already says.
        </p>
      </div>
      <form
        onSubmit={(e) => {
          e.preventDefault();
          importResume.mutate();
        }}
        className="space-y-3"
      >
        <textarea
          className="min-h-40 w-full rounded-lg border border-zinc-700 bg-zinc-900 px-3 py-2 text-sm"
          placeholder="Paste # Name — Career Evidence Bank …"
          value={text}
          onChange={(e) => setText(e.target.value)}
          required
        />
        <button
          disabled={importResume.isPending}
          className="rounded-lg bg-indigo-400 px-4 py-2 text-sm font-semibold text-zinc-950 disabled:opacity-50"
        >
          {importResume.isPending ? "Importing…" : "Import Markdown"}
        </button>
      </form>
      {importResume.isError && (
        <p className="text-sm text-red-300">
          {importResume.error instanceof Error ? importResume.error.message : "import failed"}
        </p>
      )}
      {importResume.isSuccess && importResume.data && (
        <p className="text-sm text-emerald-300">
          Imported {importResume.data.items} items, {importResume.data.accomplishments}{" "}
          accomplishments, {importResume.data.skills} skills. Jobs are rescoring.
        </p>
      )}
      {profile.isError && (
        <p className="text-sm text-red-300">
          {profile.error instanceof Error ? profile.error.message : "failed"}
        </p>
      )}
      {profile.data && (
        <section className="space-y-4">
          <div>
            <h2 className="text-xl font-semibold">
              {profile.data.full_name ?? profile.data.name}
            </h2>
            <p className="text-sm text-zinc-400">
              {profile.data.headline ?? "Default profile"}
              {profile.data.location ? ` · ${profile.data.location}` : ""}
              {profile.data.accepts_remote ? " · remote" : ""}
              {profile.data.years_experience != null
                ? ` · ${profile.data.years_experience.toFixed(1)}y`
                : ""}
              {profile.data.target_comp_min_cents != null
                ? ` · ≥$${(profile.data.target_comp_min_cents / 100).toLocaleString()}`
                : ""}
            </p>
            {profile.data.target_titles.length > 0 && (
              <p className="mt-1 text-sm text-zinc-500">
                {profile.data.target_titles.join(" · ")}
              </p>
            )}
            {profile.data.summary_md && (
              <p className="mt-3 text-sm text-zinc-300">{profile.data.summary_md}</p>
            )}
          </div>
          {profile.data.skills.filter((s) => s.is_primary).length > 0 && (
            <div>
              <h3 className="text-sm font-medium text-zinc-400">Primary skills</h3>
              <p className="mt-1 text-sm text-zinc-200">
                {profile.data.skills
                  .filter((s) => s.is_primary)
                  .map((s) =>
                    s.years != null ? `${s.slug} ${s.years.toFixed(1)}y` : s.slug,
                  )
                  .join(" · ")}
              </p>
            </div>
          )}
          <div className="space-y-5">
            {profile.data.experience.map((item) => (
              <article key={item.id}>
                <h3 className="font-medium">
                  {item.title ?? item.kind} — {item.org}
                </h3>
                <p className="text-sm text-zinc-500">
                  {item.start_date ?? "?"}
                  {" → "}
                  {item.is_current ? "present" : (item.end_date ?? "?")}
                </p>
                <ul className="mt-2 list-disc space-y-1 pl-5 text-sm text-zinc-300">
                  {item.accomplishments.map((acc) => (
                    <li key={acc.id}>
                      {acc.text}
                      {acc.verified ? (
                        <span className="ml-2 text-xs text-emerald-400">verified</span>
                      ) : null}
                    </li>
                  ))}
                </ul>
              </article>
            ))}
          </div>
        </section>
      )}
    </div>
  );
}
