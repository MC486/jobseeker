import {
  Navigate,
  createRootRoute,
  createRoute,
  createRouter,
} from "@tanstack/react-router";
import {
  Home,
  JobList,
  JobPage,
  ProfilePage,
  RootLayout,
  TaskPage,
  type JobSort,
} from "./App";

export type JobsSearch = {
  q?: string;
  sort?: JobSort;
};

function parseJobsSearch(search: Record<string, unknown>): JobsSearch {
  const q = typeof search.q === "string" && search.q.trim() ? search.q : undefined;
  const sort =
    search.sort === "skills" || search.sort === "years" || search.sort === "overall"
      ? search.sort
      : undefined;
  return { q, sort };
}

const rootRoute = createRootRoute({
  component: RootLayout,
  notFoundComponent: () => <Navigate to="/" />,
});

const indexRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/",
  component: Home,
});

const jobsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/jobs",
  validateSearch: parseJobsSearch,
  component: JobList,
});

const jobRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/jobs/$jobId",
  component: JobPage,
});

const profileRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/profile",
  component: ProfilePage,
});

const taskRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/tasks/$taskId",
  component: TaskPage,
});

const routeTree = rootRoute.addChildren([
  indexRoute,
  jobsRoute,
  jobRoute,
  profileRoute,
  taskRoute,
]);

export const router = createRouter({ routeTree });

declare module "@tanstack/react-router" {
  interface Register {
    router: typeof router;
  }
}
