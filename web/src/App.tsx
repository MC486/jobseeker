import { FormEvent, useEffect, useMemo, useState } from "react";
import { api, JobDetail, JobListRow, TaskView } from "./api";

type Route =
  | { name: "home" }
  | { name: "jobs" }
  | { name: "job"; id: string }
  | { name: "task"; id: string };

function parseRoute(): Route {
  const path = window.location.pathname.replace(/\/+$/, "") || "/";
  if (path === "/") return { name: "home" };
  if (path === "/jobs") return { name: "jobs" };
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
  useEffect(() => {
    const onPop = () => setRoute(parseRoute());
    window.addEventListener("popstate", onPop);
    return () => window.removeEventListener("popstate", onPop);
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
            <a href="/openapi.json" className="hover:text-zinc-100">
              OpenAPI
            </a>
          </nav>
        </div>
      </header>
      <main className="mx-auto max-w-4xl px-5 py-8">
        {route.name === "home" && <Home />}
        {route.name === "jobs" && <JobList />}
        {route.name === "job" && <JobPage id={route.id} />}
        {route.name === "task" && <TaskPage id={route.id} />}
      </main>
    </div>
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

function JobList() {
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
  }, [q]);

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
            </button>
          </li>
        ))}
      </ul>
      {rows.length === 0 && !error && <p className="text-zinc-500">No jobs yet.</p>}
    </div>
  );
}

function JobPage({ id }: { id: string }) {
  const [job, setJob] = useState<JobDetail | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    api
      .job(id)
      .then(setJob)
      .catch((err: unknown) => setError(err instanceof Error ? err.message : "failed"));
  }, [id]);
  if (error) return <p className="text-red-300">{error}</p>;
  if (!job) return <p className="text-zinc-500">Loading…</p>;
  return (
    <article className="space-y-6">
      <div>
        <p className="text-sm text-zinc-400">{job.company_name}</p>
        <h1 className="text-2xl font-semibold">{job.title}</h1>
        <p className="mt-1 text-sm text-zinc-400">
          {job.work_mode} · {job.seniority} · {job.employment_type}
          {job.extraction_partial ? " · partial extraction" : ""}
        </p>
        {job.apply_url && (
          <a className="mt-2 inline-block text-sm text-indigo-300" href={job.apply_url}>
            Apply
          </a>
        )}
      </div>
      {job.salary_raw && <p>Comp: {job.salary_raw}</p>}
      {job.locations.length > 0 && (
        <p className="text-zinc-300">Location: {job.locations.join(" · ")}</p>
      )}
      <section>
        <h2 className="mb-2 text-lg font-medium">Requirements</h2>
        <ul className="space-y-1 text-sm">
          {job.requirements.map((r) => (
            <li key={r.id}>
              <span className="text-zinc-500">{r.necessity}</span> {r.text}
              {r.is_blocker ? " · blocker" : ""}
            </li>
          ))}
        </ul>
      </section>
      <section>
        <h2 className="mb-2 text-lg font-medium">Description</h2>
        <pre className="whitespace-pre-wrap text-sm text-zinc-300">{job.description_md}</pre>
      </section>
    </article>
  );
}

function TaskPage({ id }: { id: string }) {
  const [task, setTask] = useState<TaskView | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let n = 0;
    const tick = () => {
      api
        .task(id)
        .then(setTask)
        .catch((err: unknown) => setError(err instanceof Error ? err.message : "failed"));
    };
    tick();
    const handle = window.setInterval(() => {
      n += 1;
      if (n < 30) tick();
    }, 1000);
    return () => window.clearInterval(handle);
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
