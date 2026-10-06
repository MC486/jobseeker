import { createFileRoute } from "@tanstack/react-router";
import { JobList } from "../App";
import { parseJobsSearch } from "../facets";

export const Route = createFileRoute("/jobs")({
  validateSearch: parseJobsSearch,
  component: JobList,
});
