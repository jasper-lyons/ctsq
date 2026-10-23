import { Card } from './card';

function Widget({ title, items }) {
  const [first, ...rest] = items;
  return (
    <Card.Body heading={title}>
      {rest.map((it) => <Line key={it} />)}
      {format(first)}
    </Card.Body>
  );
}

function format(x) {
  return String(x);
}

export default Widget;
