import type { RouteDefinition } from "@solidjs/router";
import { EventPage, NewEvent, Team, Docs, Settings, Org, Child, Guard, Member } from "./pages";

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
    children: [
      { path: "/child/:id", component: Child },
      {
        component: Guard,
        children: [
          {
            path: "/squad/:squad",
            component: Team,
            children: [{ path: "/member/:member", component: Member }],
          },
        ],
      },
    ],
  },
];
