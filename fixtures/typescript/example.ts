import { readFile } from "fs";

export enum Color {
  Red,
  Green,
  Blue,
}

export interface IUser {
  id: number;
  name: string;
  greet(): string;
}

export type Point = { x: number; y: number };

declare function ambient(x: string): void;

export function greet(user: IUser): string {
  return `Hello, ${user.name}`;
}

const add = (a: number, b: number): number => a + b;

export abstract class Animal {
  abstract speak(): void;
  move(distance: number): void {
    console.log(`Moved ${distance}m`);
  }
}

export class Dog extends Animal {
  speak(): void {
    console.log("Woof!");
  }
}

export const defaults = {
  retries: 3,
  label: "ctsq",
};
