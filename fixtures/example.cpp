#include <cstdlib>
#include <string>

class Buffer {
public:
    Buffer(int size) : size_(size) {
        data_ = malloc(size);
    }

    ~Buffer() {
        free(data_);
    }

    void process(int ARRAY_SIZE) {
        char *buf = (char *)malloc(ARRAY_SIZE);
        free(buf);
    }

private:
    void *data_;
    int size_;
};

int main(int argc, char **argv) {
    Buffer b(64);
    b.process(64);
    return 0;
}
