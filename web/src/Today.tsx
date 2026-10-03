import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { JobListRow } from "./api";
import { fetchers, keys } from "./query";

export function TodayPage() {
  const board = useQuery({ queryKey: keys.today, queryFn: fetchers.today });
  const data = board.data;
  const empty =
    data != null &&
    data.overdue.length === 0 &&
    data.due_soon.length === 0 &&
    data.possibly_ghosted.length === 0 &&
    data.closing_soon.length === 0;

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-2xl font-semibold">Today</h1>
        <p className="mt-1 text-sm text-zinc-500">
          {data ? data.today : "Loading…"} · overdue follow-ups, the next 7
          days, silent applications, and jobs closing with no application.
        </p>
      </div>
      {board.isError ? (
        <p className="text-sm text-red-300">
          {board.error instanceof Error ? board.error.message : "failed"}
        </p>
      ) : null}
      {data?.truncated ? (
        <p className="text-xs text-zinc-500">
          A later page of jobs exists, so some rows are not on this list.
        </p>
      ) : null}
      {empty ? (
        <p className="text-sm text-zinc-500">
          Nothing due. Set a next action on a job, or ingest a posting that is
          about to close.
        </p>
      ) : null}
      {data ? (
        <>
          <TodaySection
            title="Overdue"
            tone="text-red-300"
            rows={data.overdue}
            detail={(row) =>
              `overdue ${row.next_action_due ?? ""} · ${row.next_action ?? "next"}`
            }
          />
          <TodaySection
            title="Due in 7 days"
            tone="text-amber-300"
            rows={data.due_soon}
            detail={(row) =>
              `due ${row.next_action_due ?? ""} · ${row.next_action ?? "next"}`
            }
          />
          <TodaySection
            title="Possibly ghosted"
            tone="text-amber-300"
            rows={data.possibly_ghosted}
            detail={(row) => row.application_status ?? "applied"}
          />
          <TodaySection
            title="Closing soon, no application"
            tone="text-amber-300"
            rows={data.closing_soon}
            detail={(row) => `closes ${(row.closes_at ?? "").slice(0, 10) || "soon"}`}
          />
        </>
      ) : null}
    </div>
  );
}

function TodaySection({
  title,
  tone,
  rows,
  detail,
}: {
  title: string;
  tone: string;
  rows: JobListRow[];
  detail: (row: JobListRow) => string;
}) {
  if (rows.length === 0) return null;
  return (
    <section className="space-y-2">
      <h2 className={`text-sm font-medium ${tone}`}>
        {title} · {rows.length}
      </h2>
      <ul className="divide-y divide-zinc-800 rounded-xl border border-zinc-800">
        {rows.map((row) => (
          <li key={row.id}>
            <Link
              to="/jobs/$jobId"
              params={{ jobId: row.id }}
              className="block px-3 py-2 hover:bg-zinc-900"
            >
              <div className="font-medium">{row.title}</div>
              <div className="text-sm text-zinc-400">
                {row.company_name}
                {row.application_status ? ` · ${row.application_status}` : ""}
              </div>
              <div className={`text-xs ${tone}`}>{detail(row)}</div>
            </Link>
          </li>
        ))}
      </ul>
    </section>
  );
}
