import { Button } from "./Button";

export function Card(props: { name: string }) {
  return (
    <section title="Profile card">
      <h2>Welcome back</h2>
      <input placeholder="Search people" aria-label={"Find a person"} />
      <img alt="Avatar" src={props.name} />
      <Button label="Save changes" tooltip="Stores the card" />
    </section>
  );
}

export const Footer = () => <footer>All rights reserved</footer>;

export const Panel = (props: { id: string }) => {
  if (!props.id) return null;
  return <div>Panel {props.id}</div>;
};

export const Spacer = () => <Divider />;

export function helper(x: number) {
  return x + 1;
}

const NotAComponent = () => {
  const x = 1;
  return x;
};
