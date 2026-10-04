import { createFileRoute } from "@tanstack/react-router";
import { JobOverview } from "../App";

export const Route = createFileRoute("/jobs_/$jobId/")({
  component: JobOverview,
});
