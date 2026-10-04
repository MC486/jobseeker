import { createFileRoute } from "@tanstack/react-router";
import { JobSources } from "../App";

export const Route = createFileRoute("/jobs_/$jobId/sources")({
  component: JobSources,
});
