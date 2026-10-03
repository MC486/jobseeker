import { Navigate, createRootRoute } from "@tanstack/react-router";
import { RootLayout } from "../App";

export const Route = createRootRoute({
  component: RootLayout,
  notFoundComponent: () => <Navigate to="/" />,
});
