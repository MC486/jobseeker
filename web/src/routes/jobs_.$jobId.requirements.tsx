import { createFileRoute } from "@tanstack/react-router";
import { JobRequirements } from "../App";

export const Route = createFileRoute("/jobs_/$jobId/requirements")({
  component: JobRequirements,
});
