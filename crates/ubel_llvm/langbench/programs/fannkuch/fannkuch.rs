// fannkuch-redux: see fannkuch.ubl. Same algorithm. Safe Rust, bounds-checked indexing.

fn main() {
    let n: usize = @N@;
    let mut perm1: Vec<i64> = (0..n as i64).collect();
    let mut perm: Vec<i64> = vec![0; n];
    let mut count: Vec<i64> = vec![0; n];
    let (mut max_flips, mut perm_count, mut checksum) = (0i64, 0i64, 0i64);
    let mut r = n;
    let mut running = true;
    while running {
        while r != 1 {
            count[r - 1] = r as i64;
            r -= 1;
        }
        for i in 0..n {
            perm[i] = perm1[i];
        }
        let mut flips = 0i64;
        let mut k = perm[0] as usize;
        while k != 0 {
            let (mut i, mut j) = (0usize, k);
            while i < j {
                let t = perm[i];
                perm[i] = perm[j];
                perm[j] = t;
                i += 1;
                j -= 1;
            }
            flips += 1;
            k = perm[0] as usize;
        }
        if flips > max_flips {
            max_flips = flips;
        }
        if perm_count % 2 == 0 {
            checksum += flips;
        } else {
            checksum -= flips;
        }
        let mut more = true;
        while more {
            if r == n {
                running = false;
                more = false;
            } else {
                let p0 = perm1[0];
                let mut i = 0;
                while i < r {
                    perm1[i] = perm1[i + 1];
                    i += 1;
                }
                perm1[r] = p0;
                count[r] -= 1;
                if count[r] > 0 {
                    more = false;
                } else {
                    r += 1;
                }
            }
        }
        perm_count += 1;
    }
    println!("{}", checksum);
    println!("{}", max_flips);
}
