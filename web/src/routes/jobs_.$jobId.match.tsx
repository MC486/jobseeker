import { createFileRoute } from "@tanstack/react-router";
import { JobMatch } from "../App";

export const Route = createFileRoute("/jobs_/$jobId/match")({
  component: JobMatch,
});
