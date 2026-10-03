import { createFileRoute } from "@tanstack/react-router";
import { JobPage } from "../App";

export const Route = createFileRoute("/jobs_/$jobId")({
  component: JobPage,
});
