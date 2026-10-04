import { createFileRoute } from "@tanstack/react-router";
import { JobDescription } from "../App";

export type DescriptionSearch = {
  req?: string;
};

function parseDescriptionSearch(search: Record<string, unknown>): DescriptionSearch {
  const req = typeof search.req === "string" && search.req.trim() ? search.req : undefined;
  return { req };
}

export const Route = createFileRoute("/jobs_/$jobId/description")({
  validateSearch: parseDescriptionSearch,
  component: JobDescription,
});
