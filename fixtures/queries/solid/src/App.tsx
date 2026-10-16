import { Router, Route } from "@solidjs/router";
import { Home } from "./pages/Home";
import { About } from "./pages/About";

export const app = (
  <Router>
    <Route path="/" component={Home} />
    <Route path="/about" component={About} />
  </Router>
);
