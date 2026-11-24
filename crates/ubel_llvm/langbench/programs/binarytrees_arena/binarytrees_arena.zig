// binary-trees on an arena: see binarytrees.ubl for the problem. std.heap.ArenaAllocator over the page allocator; each
// loop iteration builds its tree in the same arena and resets it afterwards (retaining the memory).
const std = @import("std");

const Node = struct {
    l: ?*Node,
    r: ?*Node,
};

fn build(a: std.mem.Allocator, depth: i64) *Node {
    const n = a.create(Node) catch unreachable;
    if (depth == 0) {
        n.* = .{ .l = null, .r = null };
    } else {
        const l = build(a, depth - 1);
        const r = build(a, depth - 1);
        n.* = .{ .l = l, .r = r };
    }
    return n;
}

fn check(t: ?*const Node) i64 {
    if (t) |n| return 1 + check(n.l) + check(n.r);
    return 0;
}

pub fn main() !void {
    const out = std.io.getStdOut().writer();
    const n: i64 = @N@;
    const min_depth: i64 = 4;
    var max_depth: i64 = min_depth + 2;
    if (n > max_depth) max_depth = n;
    const stretch = max_depth + 1;
    {
        var s = std.heap.ArenaAllocator.init(std.heap.page_allocator);
        defer s.deinit();
        try out.print("stretch tree of depth {d} check: {d}\n", .{ stretch, check(build(s.allocator(), stretch)) });
    }
    var long_arena = std.heap.ArenaAllocator.init(std.heap.page_allocator);
    defer long_arena.deinit();
    const long_lived = build(long_arena.allocator(), max_depth);
    var work = std.heap.ArenaAllocator.init(std.heap.page_allocator);
    defer work.deinit();
    var d = min_depth;
    while (d <= max_depth) : (d += 2) {
        var iterations: i64 = 1;
        var k: i64 = 0;
        while (k < max_depth - d + min_depth) : (k += 1) iterations *= 2;
        var total: i64 = 0;
        var i: i64 = 0;
        while (i < iterations) : (i += 1) {
            total += check(build(work.allocator(), d));
            _ = work.reset(.retain_capacity);
        }
        try out.print("{d} trees of depth {d} check: {d}\n", .{ iterations, d, total });
    }
    try out.print("long lived tree of depth {d} check: {d}\n", .{ max_depth, check(long_lived) });
}
