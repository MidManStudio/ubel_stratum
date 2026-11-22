/* binary-trees: see binarytrees.ubl. Every node is malloc'd and freed by hand, the usual C version.
 * A NULL child is the empty tree (the Ubel enum's payload-free variant is a static object, not an allocation). */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

typedef struct Node { struct Node *l, *r; } Node;

static Node *make(int64_t depth) {
    Node *n = malloc(sizeof(Node));
    if (depth == 0) { n->l = NULL; n->r = NULL; }
    else { n->l = make(depth - 1); n->r = make(depth - 1); }
    return n;
}

static int64_t check(const Node *t) {
    if (t == NULL) return 0;
    return 1 + check(t->l) + check(t->r);
}

static void release(Node *t) {
    if (t == NULL) return;
    release(t->l);
    release(t->r);
    free(t);
}

int main(void) {
    int64_t n = @N@;
    int64_t min_depth = 4, max_depth = min_depth + 2;
    if (n > max_depth) max_depth = n;
    int64_t stretch = max_depth + 1;
    Node *s = make(stretch);
    printf("stretch tree of depth %lld check: %lld\n", (long long)stretch, (long long)check(s));
    release(s);
    Node *long_lived = make(max_depth);
    for (int64_t d = min_depth; d <= max_depth; d += 2) {
        int64_t iterations = 1;
        for (int64_t k = 0; k < max_depth - d + min_depth; k++) iterations *= 2;
        int64_t total = 0;
        for (int64_t i = 0; i < iterations; i++) {
            Node *t = make(d);
            total += check(t);
            release(t);
        }
        printf("%lld trees of depth %lld check: %lld\n", (long long)iterations, (long long)d, (long long)total);
    }
    printf("long lived tree of depth %lld check: %lld\n", (long long)max_depth, (long long)check(long_lived));
    release(long_lived);
    return 0;
}
