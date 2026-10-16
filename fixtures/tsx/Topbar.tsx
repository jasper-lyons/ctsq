// A const arrow component: the name lives in the binding, not on the function.
// Kept free of calls and JSX elements so the pinned call graph over fixtures/ is unchanged.
export const Topbar = ({ title }: { title: string }) => {
  const label = title;
  return label;
};

export const panelProps = { className: "panel", retries: 3 };
