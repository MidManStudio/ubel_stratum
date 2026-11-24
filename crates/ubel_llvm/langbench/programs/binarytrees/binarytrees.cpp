// binary-trees: see binarytrees.ubl. new/delete nodes, released by hand. nullptr is the empty tree.
#include <cstdint>
#include <cstdio>

struct Node {
    Node *l, *r;
};

static Node *build(int64_t depth) {
    Node *n = new Node{nullptr, nullptr};
    if (depth > 0) {
        n->l = build(depth - 1);
        n->r = build(depth - 1);
    }
    return n;
}

static int64_t check(const Node *t) {
    if (t == nullptr) return 0;
    return 1 + check(t->l) + check(t->r);
}

static void release(Node *t) {
    if (t == nullptr) return;
    release(t->l);
    release(t->r);
    delete t;
}

int main() {
    int64_t n = @N@;
    int64_t min_depth = 4, max_depth = min_depth + 2;
    if (n > max_depth) max_depth = n;
    int64_t stretch = max_depth + 1;
    Node *s = build(stretch);
    std::printf("stretch tree of depth %lld check: %lld\n", (long long)stretch, (long long)check(s));
    release(s);
    Node *long_lived = build(max_depth);
    for (int64_t d = min_depth; d <= max_depth; d += 2) {
        int64_t iterations = 1;
        for (int64_t k = 0; k < max_depth - d + min_depth; k++) iterations *= 2;
        int64_t total = 0;
        for (int64_t i = 0; i < iterations; i++) {
            Node *t = build(d);
            total += check(t);
            release(t);
        }
        std::printf("%lld trees of depth %lld check: %lld\n", (long long)iterations, (long long)d, (long long)total);
    }
    std::printf("long lived tree of depth %lld check: %lld\n", (long long)max_depth, (long long)check(long_lived));
    release(long_lived);
    return 0;
}
