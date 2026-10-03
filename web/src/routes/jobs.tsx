import { createFileRoute } from "@tanstack/react-router";
import { JobList, type JobSort } from "../App";

export type JobsSearch = {
  q?: string;
  sort?: JobSort;
};

function parseJobsSearch(search: Record<string, unknown>): JobsSearch {
  const q = typeof search.q === "string" && search.q.trim() ? search.q : undefined;
  const sort =
    search.sort === "skills" || search.sort === "years" || search.sort === "overall"
      ? search.sort
      : undefined;
  return { q, sort };
}

export const Route = createFileRoute("/jobs")({
  validateSearch: parseJobsSearch,
  component: JobList,
});
