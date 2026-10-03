import { useEffect, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { api, JobListRow } from "./api";
import { fetchers, keys } from "./query";

const COLUMNS = [
  "interested",
  "preparing",
  "applied",
  "screening",
  "interviewing",
  "offer",
  "accepted",
  "rejected",
  "withdrawn",
  "ghosted",
] as const;

type Column = (typeof COLUMNS)[number];

function utcToday(): string {
  return new Date().toISOString().slice(0, 10);
}

function pct(value: number | null): string {
  return value == null ? "" : `${Math.round(value * 100)}%`;
}

export function PipelinePage() {
  const qc = useQueryClient();
  const jobs = useQuery({ queryKey: keys.pipeline, queryFn: fetchers.pipeline });
  const [over, setOver] = useState<Column | null>(null);
  const [error, setError] = useState<string | null>(null);
  const dragId = useRef<string | null>(null);
  const overRef = useRef<Column | null>(null);
  const dropRef = useRef<(status: Column, jobId: string) => void>(() => {});
  const move = useMutation({
    mutationFn: ({ jobId, status }: { jobId: string; status: string }) =>
      api.patchApplication(jobId, { status }),
    onMutate: async ({ jobId, status }) => {
      await qc.cancelQueries({ queryKey: keys.pipeline });
      const prev = qc.getQueryData<typeof jobs.data>(keys.pipeline);
      qc.setQueryData<typeof jobs.data>(keys.pipeline, (page) => {
        if (!page) return page;
        return {
          ...page,
          items: page.items.map((row) =>
            row.id === jobId
              ? {
                  ...row,
                  application_status: status,
                  possibly_ghosted: false,
                  closing_soon_unapplied:
                    status === "interested" || status === "preparing"
                      ? row.closing_soon_unapplied
                      : false,
                }
              : row,
          ),
        };
      });
      return { prev };
    },
    onSuccess: (row, { jobId }) => {
      qc.setQueryData(keys.application(jobId), row);
      qc.invalidateQueries({ queryKey: ["jobs"] });
      qc.invalidateQueries({ queryKey: keys.pipeline });
      qc.invalidateQueries({ queryKey: keys.job(jobId) });
      setError(null);
    },
    onError: (err, _vars, ctx) => {
      if (ctx?.prev) qc.setQueryData(keys.pipeline, ctx.prev);
      setError(err instanceof Error ? err.message : String(err));
    },
  });

  const tracked = (jobs.data?.items ?? []).filter(
    (row): row is JobListRow & { application_status: string } =>
      row.application_status != null && row.application_status !== "",
  );
  const byStatus = new Map<string, JobListRow[]>();
  for (const status of COLUMNS) byStatus.set(status, []);
  for (const row of tracked) {
    const bucket = byStatus.get(row.application_status);
    if (bucket) bucket.push(row);
  }

  function hover(status: Column | null) {
    overRef.current = status;
    setOver(status);
  }

  function dropOn(status: Column, jobId: string) {
    const row = tracked.find((item) => item.id === jobId);
    if (!row || row.application_status === status) return;
    move.mutate({ jobId, status });
  }
  dropRef.current = dropOn;

  useEffect(() => {
    function onMove(e: PointerEvent) {
      if (!dragId.current) return;
      hover(columnAt(e.clientX, e.clientY));
    }
    function onUp(e: PointerEvent) {
      const jobId = dragId.current;
      if (!jobId) return;
      dragId.current = null;
      const next = columnAt(e.clientX, e.clientY) ?? overRef.current;
      hover(null);
      if (next) dropRef.current(next, jobId);
    }
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
    return () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
    };
  }, []);

  return (
    <div className="space-y-4">
      <div>
        <h1 className="text-2xl font-semibold">Pipeline</h1>
        <p className="mt-1 text-sm text-zinc-500">
          Drag a card to change where you are with that application. Posting
          liveness stays on the job.
        </p>
      </div>
      {error ? <p className="text-sm text-red-300">{error}</p> : null}
      {jobs.isError ? (
        <p className="text-sm text-red-300">
          {jobs.error instanceof Error ? jobs.error.message : "failed"}
        </p>
      ) : null}
      {jobs.data?.next_cursor ? (
        <p className="text-xs text-zinc-500">
          A later page of jobs exists, so some rows are not on this board.
        </p>
      ) : null}
      {!jobs.isLoading && tracked.length === 0 ? (
        <p className="text-sm text-zinc-500">
          Nothing tracked yet. Set a pipeline status on a job to put it here.
        </p>
      ) : null}
      <div className="flex overflow-x-auto pb-4">
        {COLUMNS.map((status) => {
          const cards = byStatus.get(status) ?? [];
          const hot = over === status;
          return (
            <section key={status} data-column={status} className="w-60 shrink-0 px-1.5">
              <div
                className={`min-h-64 rounded-xl border bg-zinc-900/40 ${
                  hot ? "border-indigo-400" : "border-zinc-800"
                }`}
              >
                <header className="flex items-baseline justify-between px-3 py-2">
                  <h2 className="text-sm font-medium capitalize text-zinc-200">{status}</h2>
                  <span className="text-xs text-zinc-500" aria-label={`${cards.length} in ${status}`}>
                    {cards.length}
                  </span>
                </header>
                <ul className="min-h-40 space-y-2 px-2 pb-3">
                  {cards.map((row) => (
                    <PipelineCard
                      key={row.id}
                      row={row}
                      pending={move.isPending}
                      onDragStart={() => {
                        dragId.current = row.id;
                      }}
                      onMove={(next) => dropOn(next, row.id)}
                    />
                  ))}
                </ul>
              </div>
            </section>
          );
        })}
      </div>
    </div>
  );
}

function columnAt(x: number, y: number): Column | null {
  const raw = document.elementFromPoint(x, y)?.closest("[data-column]")?.getAttribute("data-column");
  return COLUMNS.find((status) => status === raw) ?? null;
}

function PipelineCard({
  row,
  pending,
  onDragStart,
  onMove,
}: {
  row: JobListRow;
  pending: boolean;
  onDragStart: () => void;
  onMove: (status: Column) => void;
}) {
  const today = utcToday();
  const overdue = Boolean(row.next_action_due && row.next_action_due < today);
  return (
    <li
      onPointerDown={(e) => {
        if ((e.target as HTMLElement).closest("a, select, label")) return;
        onDragStart();
      }}
      className="cursor-grab rounded-lg border border-zinc-800 bg-zinc-950 p-2 active:cursor-grabbing"
    >
      <p className="text-[10px] uppercase tracking-wide text-zinc-600">Drag</p>
      <Link
        draggable={false}
        to="/jobs/$jobId"
        params={{ jobId: row.id }}
        className="text-sm font-medium text-zinc-100 hover:text-indigo-300"
      >
        {row.title}
      </Link>
      <p className="text-xs text-zinc-500">
        {row.company_name}
        {row.match_overall != null ? ` · ${pct(row.match_overall)}` : ""}
      </p>
      {row.possibly_ghosted ? (
        <p className="mt-1 text-xs text-amber-300">possibly ghosted</p>
      ) : null}
      {row.closing_soon_unapplied ? (
        <p className="mt-1 text-xs text-amber-300">
          closes {(row.closes_at ?? "").slice(0, 10) || "soon"}
        </p>
      ) : null}
      {row.next_action || row.next_action_due ? (
        <p className={`mt-1 text-xs ${overdue ? "text-red-300" : "text-zinc-500"}`}>
          {overdue ? "overdue " : ""}
          {row.next_action_due ? `${row.next_action_due} · ` : ""}
          {row.next_action ?? "next"}
        </p>
      ) : null}
      <label className="mt-2 block text-[11px] text-zinc-600">
        Move
        <select
          draggable={false}
          className="mt-0.5 block w-full rounded border border-zinc-800 bg-zinc-900 px-1 py-0.5 text-xs text-zinc-300"
          value={row.application_status ?? ""}
          disabled={pending}
          aria-label={`Move ${row.title} at ${row.company_name}`}
          onChange={(e) => {
            const next = e.target.value as Column;
            if (COLUMNS.includes(next)) onMove(next);
          }}
        >
          {COLUMNS.map((status) => (
            <option key={status} value={status}>
              {status}
            </option>
          ))}
        </select>
      </label>
    </li>
  );
}
