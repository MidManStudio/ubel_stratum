// n-body: see nbody.ubl. Same algorithm and operation order, so the results are bit for bit the same.
// Plain safe Rust: bounds-checked indexing, like the list indexing in the Ubel version.

#[derive(Clone, Copy)]
struct Body {
    x: f64,
    y: f64,
    z: f64,
    vx: f64,
    vy: f64,
    vz: f64,
    mass: f64,
}

const PI: f64 = 3.141592653589793;
const SOLAR_MASS: f64 = 4.0 * PI * PI;
const DAYS_PER_YEAR: f64 = 365.24;

fn body(x: f64, y: f64, z: f64, vx: f64, vy: f64, vz: f64, mass: f64) -> Body {
    Body { x, y, z, vx: vx * DAYS_PER_YEAR, vy: vy * DAYS_PER_YEAR, vz: vz * DAYS_PER_YEAR, mass: mass * SOLAR_MASS }
}

fn advance(b: &mut Vec<Body>, dt: f64) {
    let n = b.len();
    for i in 0..n {
        for j in (i + 1)..n {
            let dx = b[i].x - b[j].x;
            let dy = b[i].y - b[j].y;
            let dz = b[i].z - b[j].z;
            let d2 = dx * dx + dy * dy + dz * dz;
            let mag = dt / (d2 * d2.sqrt());
            b[i].vx -= dx * b[j].mass * mag;
            b[i].vy -= dy * b[j].mass * mag;
            b[i].vz -= dz * b[j].mass * mag;
            b[j].vx += dx * b[i].mass * mag;
            b[j].vy += dy * b[i].mass * mag;
            b[j].vz += dz * b[i].mass * mag;
        }
    }
    for i in 0..n {
        b[i].x += dt * b[i].vx;
        b[i].y += dt * b[i].vy;
        b[i].z += dt * b[i].vz;
    }
}

fn energy(b: &Vec<Body>) -> f64 {
    let n = b.len();
    let mut e = 0.0;
    for i in 0..n {
        e += 0.5 * b[i].mass * (b[i].vx * b[i].vx + b[i].vy * b[i].vy + b[i].vz * b[i].vz);
        for j in (i + 1)..n {
            let dx = b[i].x - b[j].x;
            let dy = b[i].y - b[j].y;
            let dz = b[i].z - b[j].z;
            e -= b[i].mass * b[j].mass / (dx * dx + dy * dy + dz * dz).sqrt();
        }
    }
    e
}

fn main() {
    let mut bodies: Vec<Body> = Vec::new();
    bodies.push(Body { x: 0.0, y: 0.0, z: 0.0, vx: 0.0, vy: 0.0, vz: 0.0, mass: SOLAR_MASS });
    bodies.push(body(4.84143144246472090e+00, -1.16032004402742839e+00, -1.03622044471123109e-01, 1.66007664274403694e-03, 7.69901118419740425e-03, -6.90460016972063023e-05, 9.54791938424326609e-04));
    bodies.push(body(8.34336671824457987e+00, 4.12479856412430479e+00, -4.03523417114321381e-01, -2.76742510726862411e-03, 4.99852801234917238e-03, 2.30417297573763929e-05, 2.85885980666130812e-04));
    bodies.push(body(1.28943695621391310e+01, -1.51111514016986312e+01, -2.23307578892655734e-01, 2.96460137564761618e-03, 2.37847173959480950e-03, -2.96589568540237556e-05, 4.36624404335156298e-05));
    bodies.push(body(1.53796971148509165e+01, -2.59193146099879641e+01, 1.79258772950371181e-01, 2.68067772490389322e-03, 1.62824170038242295e-03, -9.51592254519715870e-05, 5.15138902046611451e-05));
    let (mut px, mut py, mut pz) = (0.0, 0.0, 0.0);
    for b in bodies.iter() {
        px += b.vx * b.mass;
        py += b.vy * b.mass;
        pz += b.vz * b.mass;
    }
    bodies[0].vx = 0.0 - px / SOLAR_MASS;
    bodies[0].vy = 0.0 - py / SOLAR_MASS;
    bodies[0].vz = 0.0 - pz / SOLAR_MASS;
    println!("{}", (energy(&bodies) * 1000000000.0) as i64);
    let steps: i64 = @N@;
    for _ in 0..steps {
        advance(&mut bodies, 0.01);
    }
    println!("{}", (energy(&bodies) * 1000000000.0) as i64);
}
