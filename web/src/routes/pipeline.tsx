import { createFileRoute } from "@tanstack/react-router";
import { PipelinePage } from "../Pipeline";

export const Route = createFileRoute("/pipeline")({
  component: PipelinePage,
});
