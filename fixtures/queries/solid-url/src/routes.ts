import type { RouteDefinition } from "@solidjs/router";
import { EventPage, NewEvent, Team, Docs, Settings, Org, Child } from "./pages";

export const routes: RouteDefinition[] = [
  { path: "/events/:id", component: EventPage },
  { path: "/events/new", component: NewEvent },
  { path: "/api/org/:org_id/team/:team_id", component: Team },
  { path: "/docs/*rest", component: Docs },
  { path: "/settings/:tab?", component: Settings },
  { path: "*", component: Docs },
  {
    path: "/nested/:org",
    component: Org,
    children: [{ path: "/child/:id", component: Child }],
  },
];
