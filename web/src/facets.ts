import type { JobListRow } from "./api";

/** Sort keys for the jobs table. Reorders the loaded page only. */
export type JobSort = "overall" | "skills" | "years";

export const WORK_MODES = ["remote", "hybrid", "onsite", "unknown"] as const;
export type WorkModeFacet = (typeof WORK_MODES)[number];

/** Minimum overall match, as a percent. A row qualifies at that percent or higher. */
export const MATCH_FLOORS = [50, 70, 90] as const;
export type MatchFloor = (typeof MATCH_FLOORS)[number];

export type JobsSearch = {
  q?: string;
  sort?: JobSort;
  status?: string;
  work_mode?: WorkModeFacet;
  min?: MatchFloor;
};

const WORK_MODE_SET = new Set<string>(WORK_MODES);

export function parseJobsSearch(search: Record<string, unknown>): JobsSearch {
  const q = typeof search.q === "string" && search.q.trim() ? search.q : undefined;
  const sort =
    search.sort === "skills" || search.sort === "years" || search.sort === "overall"
      ? search.sort
      : undefined;
  const statusRaw = typeof search.status === "string" ? search.status.trim() : "";
  const status = /^[a-z][a-z0-9_]{0,31}$/.test(statusRaw) ? statusRaw : undefined;
  const work_mode =
    typeof search.work_mode === "string" && WORK_MODE_SET.has(search.work_mode)
      ? (search.work_mode as WorkModeFacet)
      : undefined;
  const minRaw =
    typeof search.min === "number"
      ? search.min
      : typeof search.min === "string"
        ? Number(search.min)
        : NaN;
  const min = MATCH_FLOORS.find((floor) => floor === minRaw);
  const out: JobsSearch = {};
  if (q) out.q = q;
  if (sort) out.sort = sort;
  if (status) out.status = status;
  if (work_mode) out.work_mode = work_mode;
  if (min) out.min = min;
  return out;
}

/** The status the table shows: pipeline when the job is tracked, otherwise the posting. */
export function shownStatus(row: JobListRow): string {
  const pipeline = row.application_status?.trim();
  return pipeline ? pipeline : row.status;
}

export type ActiveFacets = {
  status?: string;
  workMode?: string;
  min?: MatchFloor;
};

export function matchesFacets(row: JobListRow, facets: ActiveFacets): boolean {
  if (facets.status && shownStatus(row) !== facets.status) return false;
  if (facets.workMode && row.work_mode !== facets.workMode) return false;
  if (facets.min != null) {
    if (row.match_overall == null || row.match_overall < facets.min / 100) return false;
  }
  return true;
}

export type FacetOption = { value: string; count: number };

export type FacetRail = {
  status: FacetOption[];
  workMode: FacetOption[];
  match: { value: MatchFloor; count: number }[];
};

/**
 * Counts for each facet ignore that facet's own selection and respect the others,
 * so the rail still shows the alternatives.
 */
export function facetRail(rows: JobListRow[], facets: ActiveFacets): FacetRail {
  const statusCounts = new Map<string, number>();
  const modeCounts = new Map<string, number>();
  const matchCounts = new Map<MatchFloor, number>(MATCH_FLOORS.map((floor) => [floor, 0]));

  for (const row of rows) {
    if (matchesFacets(row, { ...facets, status: undefined })) {
      const status = shownStatus(row);
      statusCounts.set(status, (statusCounts.get(status) ?? 0) + 1);
    }
    if (matchesFacets(row, { ...facets, workMode: undefined })) {
      modeCounts.set(row.work_mode, (modeCounts.get(row.work_mode) ?? 0) + 1);
    }
    if (
      matchesFacets(row, { ...facets, min: undefined }) &&
      row.match_overall != null
    ) {
      for (const floor of MATCH_FLOORS) {
        if (row.match_overall >= floor / 100) {
          matchCounts.set(floor, (matchCounts.get(floor) ?? 0) + 1);
        }
      }
    }
  }

  if (facets.status && !statusCounts.has(facets.status)) {
    statusCounts.set(facets.status, 0);
  }
  if (facets.workMode && !modeCounts.has(facets.workMode)) {
    modeCounts.set(facets.workMode, 0);
  }

  const status = [...statusCounts.entries()]
    .map(([value, count]) => ({ value, count }))
    .sort((a, b) => b.count - a.count || a.value.localeCompare(b.value));

  const seen = new Set<string>();
  const workMode: FacetOption[] = [];
  for (const value of WORK_MODES) {
    if (!modeCounts.has(value) && value !== facets.workMode) continue;
    seen.add(value);
    workMode.push({ value, count: modeCounts.get(value) ?? 0 });
  }
  for (const [value, count] of modeCounts) {
    if (seen.has(value)) continue;
    workMode.push({ value, count });
  }

  return {
    status,
    workMode,
    match: MATCH_FLOORS.map((value) => ({ value, count: matchCounts.get(value) ?? 0 })),
  };
}
