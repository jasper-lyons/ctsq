import type { RouteDefinition } from '@solidjs/router';
import { lazy } from 'solid-js';

import { RequireAuth } from '~/lib/auth';

export const routes: RouteDefinition[] = [
  { path: '/', component: lazy(() => import('~/routes/index')) },
  { path: '/login', component: lazy(() => import('~/routes/login')) },
  {
    component: RequireAuth,
    children: [
      {
        path: '/w/:workspaceID',
        component: lazy(() => import('~/components/app/WorkspaceLayout')),
        children: [
          { path: '/library', component: lazy(() => import('~/routes/library')) },
          { path: '/library/:id', component: lazy(() => import('~/routes/library/[id]')) },
        ],
      },
    ],
  },
];
