// Excerpt of Mavexa frontend/src/components/app/AssetsIndex.tsx, trimmed for the
// ts-model tests in src/resolve/ts.rs. Lines here and in the original:
//   line 16 here, :78 there   const Row = (props) => ...
//   line 24 here, :100 there  export const AssetsIndex
//   line 25 here, :102 there  const [query, setQuery] = createSignal('')
//   line 28 here, :105 there  on(query, (q) => {
//   line 29 here, :106 there  setDebounced(q.trim())
//   line 43 here, :300 there  onInput={(e) => setQuery(e.currentTarget.value)}
//   line 49 here, :343 there  <For each={group.rows}>{(a) => <Row a={a} />}</For>
import { type Component, For, createEffect, createSignal, on, onCleanup } from 'solid-js';
import { Input } from '~/components/ui';

type WorkspaceAsset = { id: string; eventName: string };
type Props = { a: WorkspaceAsset };

const Row: Component<{ a: WorkspaceAsset }> = (props) => {
  return (
    <li data-asset-row={props.a.id}>
      <span class="truncate text-fg">{props.a.eventName}</span>
    </li>
  );
};

export const AssetsIndex: Component = () => {
  const [query, setQuery] = createSignal('');
  const [debounced, setDebounced] = createSignal('');
  createEffect(
    on(query, (q) => {
      const t = setTimeout(() => setDebounced(q.trim()), 200);
      onCleanup(() => clearTimeout(t));
    }),
  );
  const groups = () => [{ heading: 'upcoming', rows: [] as WorkspaceAsset[] }];

  return (
    <>
      <div class="relative ml-auto w-full sm:w-64">
        <Input
          class="pl-9"
          type="search"
          placeholder="Search assets"
          value={query()}
          onInput={(e) => setQuery(e.currentTarget.value)}
        />
      </div>
      <For each={groups()}>
        {(group) => (
          <ul class="divide-y divide-border-subtle rounded-2xl border border-border-subtle bg-surface">
            <For each={group.rows}>{(a) => <Row a={a} />}</For>
          </ul>
        )}
      </For>
    </>
  );
};
