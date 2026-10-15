struct User {
    email: String,
}

fn show(u: &User) -> &str {
    &u.email
}
