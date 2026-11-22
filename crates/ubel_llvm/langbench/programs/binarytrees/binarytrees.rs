// binary-trees: see binarytrees.ubl. Boxed nodes, freed when they go out of scope. `None` is the empty tree.

struct Node {
    l: Option<Box<Node>>,
    r: Option<Box<Node>>,
}

fn make(depth: i64) -> Box<Node> {
    if depth == 0 {
        Box::new(Node { l: None, r: None })
    } else {
        Box::new(Node { l: Some(make(depth - 1)), r: Some(make(depth - 1)) })
    }
}

fn check(t: &Option<Box<Node>>) -> i64 {
    match t {
        None => 0,
        Some(n) => 1 + check(&n.l) + check(&n.r),
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
    println!("stretch tree of depth {} check: {}", stretch, check(&Some(make(stretch))));
    let long_lived = Some(make(max_depth));
    let mut d = min_depth;
    while d <= max_depth {
        let mut iterations: i64 = 1;
        for _ in 0..(max_depth - d + min_depth) {
            iterations *= 2;
        }
        let mut total = 0;
        for _ in 0..iterations {
            total += check(&Some(make(d)));
        }
        println!("{} trees of depth {} check: {}", iterations, d, total);
        d += 2;
    }
    println!("long lived tree of depth {} check: {}", max_depth, check(&long_lived));
}
