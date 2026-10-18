import { createBrowserRouter, Routes, Route } from "react-router-dom";
import { Org, Team, Files, Home, Admin, User } from "./pages";

export const router = createBrowserRouter([
  { path: "/", element: <Home /> },
  {
    path: "/org/:org",
    element: <Org />,
    children: [
      { path: "team/:team", element: <Team /> },
      { path: "files/*", Component: Files },
    ],
  },
]);

export const admin = (
  <Routes>
    <Route path="/admin" element={<Admin />}>
      <Route path="users/:userId" element={<User />} />
    </Route>
  </Routes>
);
