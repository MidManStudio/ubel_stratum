// binary-trees on an arena: see binarytrees.ubl for the problem. core:mem's Arena is a plain bump allocator over one buffer
// (its free_all just resets the offset); each loop iteration builds its tree in the same arena and resets it afterwards.
// (core:mem/virtual's growing arena is not used: it takes a mutex per allocation and zeroes its memory on free_all.)
package main

import "core:fmt"
import "core:mem"

Node :: struct {
	l, r: ^Node,
}

// room for one complete tree of this depth
arena_bytes :: proc(depth: i64) -> int {
	return (1 << uint(depth + 1)) * size_of(Node) + 4096
}

build :: proc(depth: i64, allocator: mem.Allocator) -> ^Node {
	n := new(Node, allocator)
	if depth > 0 {
		n.l = build(depth - 1, allocator)
		n.r = build(depth - 1, allocator)
	}
	return n
}

check :: proc(t: ^Node) -> i64 {
	if t == nil {
		return 0
	}
	return 1 + check(t.l) + check(t.r)
}

main :: proc() {
	n: i64 = @N@
	min_depth: i64 = 4
	max_depth := min_depth + 2
	if n > max_depth {
		max_depth = n
	}
	stretch := max_depth + 1
	{
		buf := make([]byte, arena_bytes(stretch))
		defer delete(buf)
		s: mem.Arena
		mem.arena_init(&s, buf)
		fmt.printfln("stretch tree of depth %d check: %d", stretch, check(build(stretch, mem.arena_allocator(&s))))
	}
	long_buf := make([]byte, arena_bytes(max_depth))
	defer delete(long_buf)
	long_arena: mem.Arena
	mem.arena_init(&long_arena, long_buf)
	long_lived := build(max_depth, mem.arena_allocator(&long_arena))
	work_buf := make([]byte, arena_bytes(max_depth))
	defer delete(work_buf)
	work: mem.Arena
	mem.arena_init(&work, work_buf)
	for d := min_depth; d <= max_depth; d += 2 {
		iterations: i64 = 1
		for k: i64 = 0; k < max_depth - d + min_depth; k += 1 {
			iterations *= 2
		}
		total: i64 = 0
		for i: i64 = 0; i < iterations; i += 1 {
			total += check(build(d, mem.arena_allocator(&work)))
			mem.arena_free_all(&work)
		}
		fmt.printfln("%d trees of depth %d check: %d", iterations, d, total)
	}
	fmt.printfln("long lived tree of depth %d check: %d", max_depth, check(long_lived))
}
