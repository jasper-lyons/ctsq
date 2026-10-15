class User {
  email = "";
  name = "";
  show() {
    return this.email;
  }
}
const u = new User();
u.email = "a";
