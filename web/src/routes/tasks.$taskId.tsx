import { createFileRoute } from "@tanstack/react-router";
import { TaskPage } from "../App";

export const Route = createFileRoute("/tasks/$taskId")({
  component: TaskPage,
});
