export function parse(text: string) {
  if (!text) throw new Error("Nothing to parse");
  throw new ValidationError(`Bad field ${text}`);
}

export const fail = () => {
  throw "plain string failure";
};

export function halt(code: number) {
  throw `halted with ${code}`;
}
