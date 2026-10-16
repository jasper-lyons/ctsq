export function renderStatus(p: { on: boolean; n: number }) {
  return (
    <div className="Not text">
      <b>{p.on ? 'Live preview' : "Offline"}</b>
      <i>{p.on && `Streaming now`}</i>
      <i>{`Count ${p.n}`}</i>
      <u>
        {"Plain literal"}
        {`Plain template`}
        {p.on ? `Template branch` : p.n ? "Nested string" : `Nested template`}
        {p.on || "Binary string"}
        {p.on || `Binary template`}
      </u>
    </div>
  );
}
