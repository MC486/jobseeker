import { createFileRoute } from "@tanstack/react-router";
import { ProfilePage } from "../App";

export const Route = createFileRoute("/profile")({
  component: ProfilePage,
});
