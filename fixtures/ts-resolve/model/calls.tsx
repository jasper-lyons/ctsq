import { createSignal } from 'solid-js';
import { api, Menu, Panel } from './lib';

type Item = { id: string };

export function load(id: string) {
  const rows = api.get<Item[]>(`/items/${id}`);
  const one = send<void>(id);
  const maybe = api?.find(id);
  const map = new Map<string, Item>();
  return { rows, one, maybe, map };
}

export const Picker = (props: { items: Item[] }) => {
  const [open, setOpen] = createSignal(false);
  const toggle = () => {
    setOpen(!open());
  };
  return (
    <Panel
      title="Pick"
      onClick={toggle}
    >
      <Menu.Item label="one" />
    </Panel>
  );
};

class Store {
  save(item: Item) {
    persist(item);
    this.flush();
  }
}

function send<T>(x: string): T {
  return x as unknown as T;
}

function persist(x: Item) {
  for (const k of Object.keys(x)) {
    let n = k.length;
  }
}
