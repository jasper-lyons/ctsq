import { createBrowserRouter } from "react-router-dom";
import { Root, About, Layout, Team } from "./pages";

export const router = createBrowserRouter([
  { path: "/", element: <Root /> },
  { path: "/about", Component: About },
  { path: "/team", element: <Layout><Team /></Layout> },
]);
