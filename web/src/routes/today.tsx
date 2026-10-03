import { createFileRoute } from "@tanstack/react-router";
import { TodayPage } from "../Today";

export const Route = createFileRoute("/today")({
  component: TodayPage,
});
