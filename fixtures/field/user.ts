interface Shape {
  email: string;
}
class User {
  email: string = "";
  show(): string {
    return this.email;
  }
}
