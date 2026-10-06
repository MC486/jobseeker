import { createFileRoute } from "@tanstack/react-router";
import { JobDocuments } from "../App";

export const Route = createFileRoute("/jobs_/$jobId/documents")({
  component: JobDocuments,
});
