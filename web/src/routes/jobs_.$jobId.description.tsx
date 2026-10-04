import { createFileRoute } from "@tanstack/react-router";
import { JobDescription } from "../App";

export const Route = createFileRoute("/jobs_/$jobId/description")({
  component: JobDescription,
});
