// binary-trees: see binarytrees.ubl. Pointer nodes on the garbage-collected heap; nil is the empty tree.
package main

import "fmt"

type Node struct{ l, r *Node }

func build(depth int64) *Node {
	n := &Node{}
	if depth > 0 {
		n.l = build(depth - 1)
		n.r = build(depth - 1)
	}
	return n
}

func check(t *Node) int64 {
	if t == nil {
		return 0
	}
	return 1 + check(t.l) + check(t.r)
}

func main() {
	n := int64(@N@)
	minDepth := int64(4)
	maxDepth := minDepth + 2
	if n > maxDepth {
		maxDepth = n
	}
	stretch := maxDepth + 1
	fmt.Printf("stretch tree of depth %d check: %d\n", stretch, check(build(stretch)))
	longLived := build(maxDepth)
	for d := minDepth; d <= maxDepth; d += 2 {
		iterations := int64(1)
		for k := int64(0); k < maxDepth-d+minDepth; k++ {
			iterations *= 2
		}
		total := int64(0)
		for i := int64(0); i < iterations; i++ {
			total += check(build(d))
		}
		fmt.Printf("%d trees of depth %d check: %d\n", iterations, d, total)
	}
	fmt.Printf("long lived tree of depth %d check: %d\n", maxDepth, check(longLived))
}
