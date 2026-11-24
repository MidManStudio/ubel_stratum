// binary-trees on an arena: see binarytrees.ubl for the problem. The nodes of one tree live in one bump arena (a Vec of
// nodes addressed by index, so no crates are needed); a whole arena is reset at once, never freed node by node.
// Each loop iteration builds its tree in the same arena and resets it afterwards.

const NIL: u32 = u32::MAX;

struct Arena {
    nodes: Vec<(u32, u32)>,
}

impl Arena {
    fn new() -> Arena {
        Arena { nodes: Vec::new() }
    }
    fn alloc(&mut self, l: u32, r: u32) -> u32 {
        self.nodes.push((l, r));
        (self.nodes.len() - 1) as u32
    }
    fn reset(&mut self) {
        self.nodes.clear();
    }
}

fn build(a: &mut Arena, depth: i64) -> u32 {
    if depth == 0 {
        a.alloc(NIL, NIL)
    } else {
        let l = build(a, depth - 1);
        let r = build(a, depth - 1);
        a.alloc(l, r)
    }
}

fn check(a: &Arena, t: u32) -> i64 {
    if t == NIL {
        0
    } else {
        let (l, r) = a.nodes[t as usize];
        1 + check(a, l) + check(a, r)
    }
}

fn main() {
    let n: i64 = @N@;
    let min_depth = 4;
    let mut max_depth = min_depth + 2;
    if n > max_depth {
        max_depth = n;
    }
    let stretch = max_depth + 1;
    {
        let mut s = Arena::new();
        let t = build(&mut s, stretch);
        println!("stretch tree of depth {} check: {}", stretch, check(&s, t));
    }
    let mut long_arena = Arena::new();
    let long_lived = build(&mut long_arena, max_depth);
    let mut work = Arena::new();
    let mut d = min_depth;
    while d <= max_depth {
        let mut iterations: i64 = 1;
        for _ in 0..(max_depth - d + min_depth) {
            iterations *= 2;
        }
        let mut total = 0;
        for _ in 0..iterations {
            let t = build(&mut work, d);
            total += check(&work, t);
            work.reset();
        }
        println!("{} trees of depth {} check: {}", iterations, d, total);
        d += 2;
    }
    println!("long lived tree of depth {} check: {}", max_depth, check(&long_arena, long_lived));
}
