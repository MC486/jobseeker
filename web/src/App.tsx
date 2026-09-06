import { FormEvent, useEffect, useMemo, useState } from "react";
import { api, JobDetail, JobListRow, MatchSummary, Me, Profile, TaskView } from "./api";
import { isTaskView, subscribeEvents } from "./api/events";

type Route =
  | { name: "home" }
  | { name: "jobs" }
  | { name: "job"; id: string }
  | { name: "task"; id: string }
  | { name: "profile" };

function parseRoute(): Route {
  const path = window.location.pathname.replace(/\/+$/, "") || "/";
  if (path === "/") return { name: "home" };
  if (path === "/jobs") return { name: "jobs" };
  if (path === "/profile") return { name: "profile" };
  const job = path.match(/^\/jobs\/([^/]+)$/);
  if (job) return { name: "job", id: job[1] };
  const task = path.match(/^\/tasks\/([^/]+)$/);
  if (task) return { name: "task", id: task[1] };
  return { name: "home" };
}

function navigate(path: string) {
  window.history.pushState({}, "", path);
  window.dispatchEvent(new PopStateEvent("popstate"));
}

export function App() {
  const [route, setRoute] = useState<Route>(parseRoute);
  const [me, setMe] = useState<Me | null>(null);
  const [live, setLive] = useState(0);
  const [tasks, setTasks] = useState<Record<string, TaskView>>({});
  useEffect(() => {
    const onPop = () => setRoute(parseRoute());
    window.addEventListener("popstate", onPop);
    return () => window.removeEventListener("popstate", onPop);
  }, []);
  useEffect(() => {
    api.me().then(setMe).catch(() => setMe(null));
  }, []);
  useEffect(() => {
    return subscribeEvents((event) => {
      if (event.kind === "task.updated" && event.entity_id && isTaskView(event.payload)) {
        setTasks((prev) => ({ ...prev, [event.entity_id as string]: event.payload as TaskView }));
      }
      if (
        event.kind === "job.created" ||
        event.kind === "job.updated" ||
        event.kind === "match.updated" ||
        event.kind === "profile.updated"
      ) {
        setLive((n) => n + 1);
      }
    });
  }, []);

  return (
    <div className="min-h-screen">
      <header className="border-b border-zinc-800">
        <div className="mx-auto flex max-w-4xl items-baseline justify-between px-5 py-4">
          <button className="text-sm font-semibold tracking-wide" onClick={() => navigate("/")}>
            jobseeker
          </button>
          <nav className="flex gap-4 text-sm text-zinc-400">
            <button onClick={() => navigate("/jobs")}>Jobs</button>
            <button onClick={() => navigate("/profile")}>Profile</button>
            <a href="/openapi.json" className="hover:text-zinc-100">
              OpenAPI
            </a>
            {me && <span title="auth.mode">auth:{me.auth_mode}</span>}
            {me?.auth_mode === "password" && me.authenticated && (
              <button
                className="hover:text-zinc-100"
                onClick={() => {
                  api.logout().finally(() =>
                    api.me().then(setMe).catch(() => setMe(null)),
                  );
                }}
              >
                Log out
              </button>
            )}
          </nav>
        </div>
      </header>
      <main className="mx-auto max-w-4xl px-5 py-8">
        {me?.auth_mode === "password" && !me.authenticated && (
          <Login onLoggedIn={setMe} />
        )}
        {!(me?.auth_mode === "password" && !me.authenticated) && route.name === "home" && (
          <Home />
        )}
        {!(me?.auth_mode === "password" && !me.authenticated) && route.name === "profile" && (
          <ProfilePage live={live} />
        )}
        {!(me?.auth_mode === "password" && !me.authenticated) && route.name === "jobs" && (
          <JobList live={live} />
        )}
        {!(me?.auth_mode === "password" && !me.authenticated) && route.name === "job" && (
          <JobPage id={route.id} live={live} />
        )}
        {!(me?.auth_mode === "password" && !me.authenticated) && route.name === "task" && (
          <TaskPage id={route.id} pushed={tasks[route.id]} />
        )}
      </main>
    </div>
  );
}

function Login({ onLoggedIn }: { onLoggedIn: (me: Me) => void }) {
  const [username, setUsername] = useState("owner");
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function onSubmit(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      onLoggedIn(await api.login(username, password));
    } catch (err) {
      setError(err instanceof Error ? err.message : "login failed");
    } finally {
      setBusy(false);
    }
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
      {error && <p className="text-sm text-red-300">{error}</p>}
      <button
        disabled={busy}
        className="rounded-lg bg-indigo-400 px-4 py-2 text-sm font-semibold text-zinc-950 disabled:opacity-50"
      >
        Sign in
      </button>
    </form>
  );
}

function Home() {
  const [url, setUrl] = useState("");
  const [paste, setPaste] = useState("");
  const [message, setMessage] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function onUrl(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    setMessage(null);
    try {
      const accepted = await api.ingestUrl(url);
      setMessage(`Queued ${accepted.task_id}`);
      setUrl("");
      navigate(`/tasks/${accepted.task_id}`);
    } catch (err) {
      setMessage(err instanceof Error ? err.message : "failed");
    } finally {
      setBusy(false);
    }
  }

  async function onPaste(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    setMessage(null);
    try {
      const accepted = await api.ingestPaste(paste);
      setMessage(`Queued ${accepted.task_id}`);
      setPaste("");
      navigate(`/tasks/${accepted.task_id}`);
    } catch (err) {
      setMessage(err instanceof Error ? err.message : "failed");
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="space-y-8">
      <div>
        <h1 className="text-2xl font-semibold">Ingest a posting</h1>
        <p className="mt-2 text-zinc-400">
          Public ATS URLs are fetched here. LinkedIn and Indeed need the browser extension —
          this server will not log in for you.
        </p>
      </div>
      <form onSubmit={onUrl} className="space-y-3">
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
      <form onSubmit={onPaste} className="space-y-3">
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

function JobList({ live }: { live: number }) {
  const [q, setQ] = useState("");
  const [rows, setRows] = useState<JobListRow[]>([]);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    api
      .jobs(q || undefined)
      .then((page) => {
        if (!cancelled) setRows(page.items);
      })
      .catch((err: unknown) => {
        if (!cancelled) setError(err instanceof Error ? err.message : "failed");
      });
    return () => {
      cancelled = true;
    };
  }, [q, live]);

  return (
    <div className="space-y-4">
      <h1 className="text-2xl font-semibold">Jobs</h1>
      <input
        className="w-full rounded-lg border border-zinc-700 bg-zinc-900 px-3 py-2"
        placeholder="Search title, company, requirements…"
        value={q}
        onChange={(e) => setQ(e.target.value)}
      />
      {error && <p className="text-sm text-red-300">{error}</p>}
      <ul className="divide-y divide-zinc-800">
        {rows.map((row) => (
          <li key={row.id} className="py-3">
            <button className="text-left" onClick={() => navigate(`/jobs/${row.id}`)}>
              <div className="font-medium">{row.title}</div>
              <div className="text-sm text-zinc-400">
                {row.company_name} · {row.work_mode} · {row.status}
                {row.primary_location ? ` · ${row.primary_location}` : ""}
              </div>
              {row.match_overall != null ? (
                <div className="mt-1 flex flex-wrap gap-x-3 text-xs text-zinc-400">
                  <span className="text-zinc-200">{pct(row.match_overall)} match</span>
                  {row.skills_coverage != null ? (
                    <span title="Required bars with tenure stripped. Does not change overall.">
                      skills {pct(row.skills_coverage)}
                    </span>
                  ) : null}
                  {row.years_fit != null ? (
                    <span title="Required year-count asks. Recruiters overfit this; postings often mean a wishlist.">
                      years {pct(row.years_fit)}
                    </span>
                  ) : null}
                </div>
              ) : null}
            </button>
          </li>
        ))}
      </ul>
      {rows.length === 0 && !error && <p className="text-zinc-500">No jobs yet.</p>}
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

function JobPage({ id, live }: { id: string; live: number }) {
  const [job, setJob] = useState<JobDetail | null>(null);
  const [match, setMatch] = useState<MatchSummary | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    let cancelled = false;
    api
      .job(id)
      .then((detail) => {
        if (!cancelled) setJob(detail);
      })
      .catch((err: unknown) => {
        if (!cancelled) setError(err instanceof Error ? err.message : "failed");
      });
    api
      .jobMatch(id)
      .then((score) => {
        if (!cancelled) setMatch(score);
      })
      .catch(() => {
        if (!cancelled) setMatch(null);
      });
    return () => {
      cancelled = true;
    };
  }, [id, live]);

  async function toggleArchive() {
    if (!job) return;
    setBusy(true);
    try {
      const next = await api.patchJob(id, { is_archived: !job.is_archived });
      setJob(next);
    } catch (err) {
      setError(err instanceof Error ? err.message : "failed");
    } finally {
      setBusy(false);
    }
  }

  if (error) return <p className="text-red-300">{error}</p>;
  if (!job) return <p className="text-zinc-500">Loading…</p>;
  const verdictByReq = new Map((match?.verdicts ?? []).map((v) => [v.requirement_id, v]));
  return (
    <article className="space-y-6">
      <div>
        <p className="text-sm text-zinc-400">{job.company_name}</p>
        <h1 className="text-2xl font-semibold">
          {job.title}
          {provenanceDot(job, "title")}
        </h1>
        <p className="mt-1 text-sm text-zinc-400">
          {job.work_mode} · {job.seniority} · {job.employment_type}
          {job.extraction_partial ? " · partial extraction" : ""}
          {job.is_archived ? " · archived" : ""}
        </p>
        {job.apply_url && (
          <a className="mt-2 inline-block text-sm text-indigo-300" href={job.apply_url}>
            Apply
          </a>
        )}
        <div className="mt-3">
          <button
            disabled={busy}
            onClick={toggleArchive}
            className="rounded-lg border border-zinc-700 px-3 py-1 text-sm text-zinc-300 disabled:opacity-50"
          >
            {job.is_archived ? "Unarchive" : "Archive"}
          </button>
        </div>
      </div>
      {match && (
        <section className="rounded-xl border border-zinc-800 bg-zinc-900/60 p-4">
          <h2 className="text-lg font-medium">
            Match {Math.round(match.overall * 100)}%
            {match.is_stale ? " · stale" : ""}
          </h2>
          <p className="mt-1 text-xs text-zinc-500">
            Overall is unchanged. Skills ignore year shortfalls when the skill is on the
            bank; Years is the tenure bar recruiters overfit and postings often treat as a
            wishlist.
          </p>
          <dl className="mt-3 grid grid-cols-2 gap-2 text-sm sm:grid-cols-4">
            <ScoreCell
              label="Skills"
              value={match.skills_coverage}
              hint="required bars, tenure stripped"
            />
            <ScoreCell
              label="Years"
              value={match.years_fit}
              hint="required year-count asks"
            />
            <ScoreCell
              label="Required"
              value={match.required_coverage}
              hint="feeds overall (includes years)"
            />
            <ScoreCell label="Preferred" value={match.preferred_coverage} />
            <ScoreCell label="Seniority" value={match.seniority_fit} />
            <ScoreCell label="Comp" value={match.comp_fit} />
            <ScoreCell label="Location" value={match.location_fit} />
          </dl>
          {match.blocker_count > 0 ? (
            <p className="mt-2 text-sm text-amber-300">
              {match.blocker_count} blocker(s) — overall is capped
            </p>
          ) : null}
        </section>
      )}
      {job.salary_raw && (
        <p>
          Comp: {job.salary_raw}
          {provenanceDot(job, "salary")}
        </p>
      )}
      {job.locations.length > 0 && (
        <p className="text-zinc-300">Location: {job.locations.join(" · ")}</p>
      )}
      <section>
        <h2 className="mb-2 text-lg font-medium">Requirements</h2>
        <ul className="space-y-2 text-sm">
          {job.requirements.map((r) => {
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
        <pre className="whitespace-pre-wrap text-sm text-zinc-300">{job.description_md}</pre>
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

function TaskPage({ id, pushed }: { id: string; pushed?: TaskView }) {
  const [task, setTask] = useState<TaskView | null>(pushed ?? null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    if (pushed && pushed.id === id) setTask(pushed);
  }, [id, pushed]);
  useEffect(() => {
    let cancelled = false;
    api
      .task(id)
      .then((next) => {
        if (!cancelled) setTask(next);
      })
      .catch((err: unknown) => {
        if (!cancelled) setError(err instanceof Error ? err.message : "failed");
      });
    return () => {
      cancelled = true;
    };
  }, [id]);

  const done = useMemo(() => task && ["done", "failed", "cancelled"].includes(task.status), [task]);

  if (error) return <p className="text-red-300">{error}</p>;
  if (!task) return <p className="text-zinc-500">Loading…</p>;
  return (
    <div className="space-y-4">
      <h1 className="text-2xl font-semibold">Task {task.kind}</h1>
      <p className="text-zinc-400">
        {task.status}
        {task.progress_message ? ` — ${task.progress_message}` : ""}
      </p>
      {task.last_error && <p className="text-red-300">{task.last_error}</p>}
      {done && (
        <button className="text-indigo-300" onClick={() => navigate("/jobs")}>
          View jobs
        </button>
      )}
    </div>
  );
}

function ProfilePage({ live }: { live: number }) {
  const [profile, setProfile] = useState<Profile | null>(null);
  const [text, setText] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let cancelled = false;
    api
      .profile()
      .then((row) => {
        if (!cancelled) setProfile(row);
      })
      .catch((err: unknown) => {
        if (!cancelled) setError(err instanceof Error ? err.message : "failed");
      });
    return () => {
      cancelled = true;
    };
  }, [live]);

  async function onImport(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(null);
    setMessage(null);
    try {
      const report = await api.importResume(text);
      setMessage(
        `Imported ${report.items} items, ${report.accomplishments} accomplishments, ${report.skills} skills. Jobs are rescoring.`,
      );
      setProfile(await api.profile());
    } catch (err) {
      setError(err instanceof Error ? err.message : "import failed");
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="space-y-8">
      <div>
        <h1 className="text-2xl font-semibold">Experience bank</h1>
        <p className="mt-2 text-sm text-zinc-400">
          Paste a Markdown evidence bank. Import replaces the default profile and rescores every
          saved job. Nothing here is invented — only what the file already says.
        </p>
      </div>
      <form onSubmit={onImport} className="space-y-3">
        <textarea
          className="min-h-40 w-full rounded-lg border border-zinc-700 bg-zinc-900 px-3 py-2 text-sm"
          placeholder="Paste # Name — Career Evidence Bank …"
          value={text}
          onChange={(e) => setText(e.target.value)}
          required
        />
        <button
          disabled={busy}
          className="rounded-lg bg-indigo-400 px-4 py-2 text-sm font-semibold text-zinc-950 disabled:opacity-50"
        >
          {busy ? "Importing…" : "Import Markdown"}
        </button>
      </form>
      {error && <p className="text-sm text-red-300">{error}</p>}
      {message && <p className="text-sm text-emerald-300">{message}</p>}
      {profile && (
        <section className="space-y-4">
          <div>
            <h2 className="text-xl font-semibold">{profile.full_name ?? profile.name}</h2>
            <p className="text-sm text-zinc-400">
              {profile.headline ?? "Default profile"}
              {profile.location ? ` · ${profile.location}` : ""}
              {profile.accepts_remote ? " · remote" : ""}
              {profile.years_experience != null
                ? ` · ${profile.years_experience.toFixed(1)}y`
                : ""}
              {profile.target_comp_min_cents != null
                ? ` · ≥$${(profile.target_comp_min_cents / 100).toLocaleString()}`
                : ""}
            </p>
            {profile.target_titles.length > 0 && (
              <p className="mt-1 text-sm text-zinc-500">{profile.target_titles.join(" · ")}</p>
            )}
            {profile.summary_md && (
              <p className="mt-3 text-sm text-zinc-300">{profile.summary_md}</p>
            )}
          </div>
          {profile.skills.filter((s) => s.is_primary).length > 0 && (
            <div>
              <h3 className="text-sm font-medium text-zinc-400">Primary skills</h3>
              <p className="mt-1 text-sm text-zinc-200">
                {profile.skills
                  .filter((s) => s.is_primary)
                  .map((s) =>
                    s.years != null ? `${s.slug} ${s.years.toFixed(1)}y` : s.slug,
                  )
                  .join(" · ")}
              </p>
            </div>
          )}
          <div className="space-y-5">
            {profile.experience.map((item) => (
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
