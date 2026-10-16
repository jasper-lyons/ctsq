import { Routes, Route } from "react-router";
import { Contact, Legacy } from "./pages";

export const app = (
  <Routes>
    <Route path="/contact" element={<Contact />} />
    <Route path="/legacy" Component={Legacy} />
  </Routes>
);
