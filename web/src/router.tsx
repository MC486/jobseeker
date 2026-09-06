import {
  Navigate,
  createRootRoute,
  createRoute,
  createRouter,
} from "@tanstack/react-router";
import {
  ComparePage,
  Home,
  JobList,
  JobPage,
  ProfilePage,
  RootLayout,
  TaskPage,
  type JobSort,
} from "./App";
import { parseCompareIds } from "./compare";

export type JobsSearch = {
  q?: string;
  sort?: JobSort;
};

export type CompareSearch = {
  ids?: string;
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

const compareRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/jobs/compare",
  validateSearch: (search: Record<string, unknown>): CompareSearch => ({
    ids: parseCompareIds(search.ids).join(",") || undefined,
  }),
  component: ComparePage,
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
  compareRoute,
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
