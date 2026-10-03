import { createFileRoute } from "@tanstack/react-router";
import { ComparePage } from "../App";
import { parseCompareIds } from "../compare";

export const Route = createFileRoute("/jobs_/compare")({
  validateSearch: (search: Record<string, unknown>) => ({
    ids: parseCompareIds(search.ids).join(",") || undefined,
  }),
  component: ComparePage,
});
