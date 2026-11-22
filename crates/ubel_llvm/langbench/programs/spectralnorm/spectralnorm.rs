// spectral-norm: see spectralnorm.ubl. Same algorithm and operation order. Safe Rust, bounds-checked indexing.

fn a(i: i64, j: i64) -> f64 {
    1.0 / (((i + j) * (i + j + 1) / 2 + i + 1) as f64)
}

fn mul_av(n: usize, v: &Vec<f64>, av: &mut Vec<f64>) {
    for i in 0..n {
        let mut s = 0.0;
        for j in 0..n {
            s += a(i as i64, j as i64) * v[j];
        }
        av[i] = s;
    }
}

fn mul_atv(n: usize, v: &Vec<f64>, atv: &mut Vec<f64>) {
    for i in 0..n {
        let mut s = 0.0;
        for j in 0..n {
            s += a(j as i64, i as i64) * v[j];
        }
        atv[i] = s;
    }
}

fn mul_atav(n: usize, v: &Vec<f64>, out: &mut Vec<f64>, tmp: &mut Vec<f64>) {
    mul_av(n, v, tmp);
    mul_atv(n, tmp, out);
}

fn main() {
    let n: usize = @N@;
    let mut u = vec![1.0f64; n];
    let mut v = vec![0.0f64; n];
    let mut tmp = vec![0.0f64; n];
    for _ in 0..10 {
        mul_atav(n, &u, &mut v, &mut tmp);
        mul_atav(n, &v, &mut u, &mut tmp);
    }
    let (mut vbv, mut vv) = (0.0, 0.0);
    for i in 0..n {
        vbv += u[i] * v[i];
        vv += v[i] * v[i];
    }
    println!("{}", ((vbv / vv).sqrt() * 1000000000.0) as i64);
}
