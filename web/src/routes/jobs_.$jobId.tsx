import { createFileRoute } from "@tanstack/react-router";
import { JobFrame } from "../App";

export const Route = createFileRoute("/jobs_/$jobId")({
  component: JobFrame,
});
