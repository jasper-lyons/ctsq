import React from "react";

interface ButtonProps {
  label: string;
  onClick: () => void;
}

const Button = ({ label, onClick }: ButtonProps) => (
  <button onClick={onClick}>{label}</button>
);

const Icon = () => <span>★</span>;

function App() {
  const handleClick = () => console.log("clicked");
  return (
    <div>
      <Button label="Click me" onClick={handleClick} />
      <Icon />
    </div>
  );
}

export default App;
