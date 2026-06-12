#include <stdlib.h>
#include <string.h>

void process(int ARRAY_SIZE) {
    /* malloc called with ARRAY_SIZE as a direct argument */
    char *buf = malloc(ARRAY_SIZE);
    free(buf);
}

int main(int argc, char **argv) {
    process(64);
    return 0;
}
