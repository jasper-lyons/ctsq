struct user {
    char *email;
    int age;
};

int get(struct user *u) {
    return u->age;
}
