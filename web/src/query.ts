import { QueryClient } from "@tanstack/react-query";
import { api } from "./api";
import { isTaskView, subscribeEvents } from "./api/events";

export const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      staleTime: 30_000,
      retry: 1,
    },
  },
});

export const keys = {
  me: ["me"] as const,
  jobs: (q?: string) => ["jobs", { q: q ?? "" }] as const,
  job: (id: string) => ["job", id] as const,
  match: (id: string) => ["match", id] as const,
  duplicates: (id: string) => ["duplicates", id] as const,
  conflicts: (id: string) => ["conflicts", id] as const,
  profile: ["profile"] as const,
  task: (id: string) => ["task", id] as const,
};

export function subscribeQueryEvents(): () => void {
  return subscribeEvents((event) => {
    switch (event.kind) {
      case "task.updated":
        if (event.entity_id && isTaskView(event.payload)) {
          queryClient.setQueryData(keys.task(event.entity_id), event.payload);
        }
        break;
      case "job.created":
      case "job.updated":
        queryClient.invalidateQueries({ queryKey: ["jobs"] });
        if (event.entity_id) {
          queryClient.invalidateQueries({ queryKey: keys.job(event.entity_id) });
          queryClient.invalidateQueries({ queryKey: ["duplicates"] });
          queryClient.invalidateQueries({ queryKey: ["conflicts"] });
        }
        break;
      case "match.updated":
        queryClient.invalidateQueries({ queryKey: ["jobs"] });
        if (event.entity_id) {
          queryClient.invalidateQueries({ queryKey: keys.match(event.entity_id) });
        }
        break;
      case "profile.updated":
        queryClient.invalidateQueries({ queryKey: keys.profile });
        queryClient.invalidateQueries({ queryKey: ["jobs"] });
        queryClient.invalidateQueries({ queryKey: ["match"] });
        break;
    }
  });
}

export const fetchers = {
  me: () => api.me(),
  jobs: (q?: string) => api.jobs(q),
  job: (id: string) => api.job(id),
  match: (id: string) => api.jobMatch(id),
  duplicates: (id: string) => api.duplicates(id),
  conflicts: (id: string) => api.conflicts(id),
  profile: () => api.profile(),
  task: (id: string) => api.task(id),
};
