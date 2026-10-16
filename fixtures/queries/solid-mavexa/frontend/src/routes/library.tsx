import { createSignal } from 'solid-js';

export default function Library() {
  const [open] = createSignal(false);
  return <div>{String(open())}</div>;
}
