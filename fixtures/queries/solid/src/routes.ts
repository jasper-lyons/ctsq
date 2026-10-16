import type { RouteDefinition } from "@solidjs/router";
import { Settings } from "./pages/Settings";

export const routes: RouteDefinition[] = [
  { path: "/settings", component: Settings },
  { path: "/about/team", component: Nowhere },
];
