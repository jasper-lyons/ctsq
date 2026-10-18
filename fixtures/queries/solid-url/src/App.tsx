import { Router, Route } from "@solidjs/router";
import { Org, Project, Member } from "./pages";

export const app = (
  <Router>
    <Route path="/jsx/:org" component={Org}>
      <Route path="/project/:project" component={Project}>
        <Route path="/" component={Member} />
      </Route>
    </Route>
  </Router>
);
