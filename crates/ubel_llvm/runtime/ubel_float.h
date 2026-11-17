/* ubel_float.h -- printing floats exactly like Rust's `{}` (Display) does, for the native runtime.
 *
 * The interpreter prints `float` (f32) and `double` (f64) with `write!(f, "{}", v)`. That is: the SHORTEST digit
 * string that reads back as the same number (nearest to the true value when several have that length), laid out
 * without an exponent, so 1e21 is `1000000000000000000000`, 1e-7 is `0.0000001`, 1.0 is `1`, -0.0 is `-0`, and
 * NaN, inf and -inf are `NaN`, `inf` and `-inf` (a NaN never has a sign).
 *
 * This is a port of the algorithm Rust itself uses when that shortest search is needed: Burger and Dybvig's
 * Dragon ("Printing floating-point numbers quickly and accurately", Fig. 3) as written in Rust's
 * `core::num::flt2dec::strategy::dragon::format_shortest`, with its decoder (`flt2dec::decoder`), scaling-factor
 * estimator and `round_up`, and a fixed-size bignum like `core::num::bignum::Big32x40` (40 limbs of 32 bits). It
 * deliberately avoids the printf `%.{n}e` and strtod shortcut: that finds the shortest correctly ROUNDED string,
 * which is not always the shortest string in the rounding interval (at a power of two the interval is
 * asymmetric). Rust's own code (Grisu first, Dragon as the fallback) gives the same digits as Dragon alone.
 *
 * Only integer arithmetic is used, so the result does not depend on the FPU mode or on contraction flags.
 * Provisional, like the rest of the backend: checked by scripts/float_diff.sh against `format!("{}")` of the
 * Rust toolchain over powers of two and ten, neighbours of both, integers, short decimals and random bit patterns.
 * The tables of 5^(2^n) were taken from Rust's source and checked to equal 5^(2^n).
 */
#ifndef UBEL_FLOAT_H
#define UBEL_FLOAT_H

#include <stdint.h>
#include <string.h>

/* Room for the longest output: a subnormal double is `0.` followed by 323 zeros and up to 17 digits, with a sign. */
#define UBEL_FLT_BUF 400

#define UBEL_BIG_LIMBS 40

typedef struct { uint32_t d[UBEL_BIG_LIMBS]; int n; } ubel_big;   /* value = sum d[i] 2^(32 i); d[i] == 0 for i >= n */

static const uint32_t ubel_pow5to16[2] = {0x86f26fc1, 0x23};
static const uint32_t ubel_pow5to32[3] = {0x85acef81, 0x2d6d415b, 0x4ee};
static const uint32_t ubel_pow5to64[5] = {0xbf6a1f01, 0x6e38ed64, 0xdaa797ed, 0xe93ff9f4, 0x184f03};
static const uint32_t ubel_pow5to128[10] = {0x2e953e01, 0x3df9909, 0xf1538fd, 0x2374e42f, 0xd3cff5ec, 0xc404dc08, 0xbccdb0da, 0xa6337f19, 0xe91f2603, 0x24e};
static const uint32_t ubel_pow5to256[19] = {0x982e7c01, 0xbed3875b, 0xd8d99f72, 0x12152f87, 0x6bde50c6, 0xcf4a6e70, 0xd595d80f, 0x26b2716e, 0xadc666b0, 0x1d153624, 0x3c42d35a, 0x63ff540e, 0xcc5573c0, 0x65f9ef17, 0x55bc28f2, 0x80dcc7f7, 0xf46eeddc, 0x5fdcefce, 0x553f7};

static const uint32_t ubel_pow10_small[10] = {1, 10, 100, 1000, 10000, 100000, 1000000, 10000000, 100000000, 1000000000};

static inline void ubel_big_from_u64(ubel_big *b, uint64_t v) {
    memset(b->d, 0, sizeof b->d);
    b->d[0] = (uint32_t)v;
    b->d[1] = (uint32_t)(v >> 32);
    b->n = 2;
}

static inline void ubel_big_mul_small(ubel_big *b, uint32_t m) {
    uint64_t carry = 0;
    for (int i = 0; i < b->n; i++) {
        uint64_t cur = (uint64_t)b->d[i] * m + carry;
        b->d[i] = (uint32_t)cur;
        carry = cur >> 32;
    }
    if (carry) b->d[b->n++] = (uint32_t)carry;
}

/* b *= 2^bits */
static inline void ubel_big_mul_pow2(ubel_big *b, int bits) {
    int limbs = bits / 32, rem = bits % 32;
    if (limbs > 0) {
        for (int i = b->n - 1; i >= 0; i--) b->d[i + limbs] = b->d[i];
        for (int i = 0; i < limbs; i++) b->d[i] = 0;
        b->n += limbs;
    }
    if (rem > 0) {
        uint32_t carry = 0;
        for (int i = 0; i < b->n; i++) {
            uint32_t v = b->d[i];
            b->d[i] = (v << rem) | carry;
            carry = v >> (32 - rem);
        }
        if (carry) b->d[b->n++] = carry;
    }
}

/* b *= the number with the given limbs */
static inline void ubel_big_mul_digits(ubel_big *b, const uint32_t *o, int on) {
    uint32_t t[UBEL_BIG_LIMBS];
    memset(t, 0, sizeof t);
    for (int i = 0; i < b->n; i++) {
        uint64_t carry = 0;
        for (int j = 0; j < on; j++) {
            uint64_t cur = (uint64_t)t[i + j] + (uint64_t)b->d[i] * o[j] + carry;
            t[i + j] = (uint32_t)cur;
            carry = cur >> 32;
        }
        t[i + on] += (uint32_t)carry;
    }
    int n = b->n + on;
    memcpy(b->d, t, sizeof t);
    while (n > 0 && b->d[n - 1] == 0) n--;
    b->n = n;
}

/* a += b */
static inline void ubel_big_add(ubel_big *a, const ubel_big *b) {
    int n = a->n > b->n ? a->n : b->n;
    uint64_t carry = 0;
    for (int i = 0; i < n; i++) {
        uint64_t cur = (uint64_t)a->d[i] + b->d[i] + carry;
        a->d[i] = (uint32_t)cur;
        carry = cur >> 32;
    }
    a->n = n;
    if (carry) a->d[a->n++] = (uint32_t)carry;
}

/* a -= b, where a >= b */
static inline void ubel_big_sub(ubel_big *a, const ubel_big *b) {
    int64_t borrow = 0;
    for (int i = 0; i < a->n; i++) {
        int64_t cur = (int64_t)a->d[i] - b->d[i] - borrow;
        if (cur < 0) { cur += ((int64_t)1 << 32); borrow = 1; } else borrow = 0;
        a->d[i] = (uint32_t)cur;
    }
    while (a->n > 0 && a->d[a->n - 1] == 0) a->n--;
}

static inline int ubel_big_cmp(const ubel_big *a, const ubel_big *b) {
    int n = a->n > b->n ? a->n : b->n;
    for (int i = n - 1; i >= 0; i--) {
        if (a->d[i] != b->d[i]) return a->d[i] < b->d[i] ? -1 : 1;
    }
    return 0;
}

/* x *= 10^n  (Rust's `dragon::mul_pow10`: the powers of five first, then the shift by n) */
static inline void ubel_big_mul_pow10(ubel_big *x, int n) {
    if (n < 8) { ubel_big_mul_small(x, ubel_pow10_small[n & 7]); return; }
    if (n & 7) ubel_big_mul_small(x, ubel_pow10_small[n & 7] >> (n & 7));
    if (n & 8) ubel_big_mul_small(x, ubel_pow10_small[8] >> 8);
    if (n & 16) ubel_big_mul_digits(x, ubel_pow5to16, 2);
    if (n & 32) ubel_big_mul_digits(x, ubel_pow5to32, 3);
    if (n & 64) ubel_big_mul_digits(x, ubel_pow5to64, 5);
    if (n & 128) ubel_big_mul_digits(x, ubel_pow5to128, 10);
    if (n & 256) ubel_big_mul_digits(x, ubel_pow5to256, 19);
    ubel_big_mul_pow2(x, n);
}

/* `flt2dec::Decoded`: the value is mant * 2^exp, and everything from (mant - minus) * 2^exp to
 * (mant + plus) * 2^exp reads back as it (the ends too when `inclusive`). */
typedef struct { uint64_t mant, minus, plus; int exp; int inclusive; } ubel_decoded;

/* `estimator::estimate_scaling_factor`: k_0 or k_0 - 1 where k = ceil(log10(mant * 2^exp)) */
static inline int ubel_flt_estimate_k(uint64_t mant, int exp) {
    int nbits = 64 - __builtin_clzll(mant - 1);
    return (int)((((int64_t)nbits + exp) * 1292913986LL) >> 32);
}

/* `a.cmp(b) < rounding` of Rust's `format_shortest`: `a <= b` when the interval ends are included, else `a < b` */
static inline int ubel_flt_lt(int cmp, int inclusive) { return inclusive ? cmp <= 0 : cmp < 0; }

/* One decimal digit of x / scale, for x < 16 * scale (`div_rem_upto_16`); x is left as the remainder. */
static inline int ubel_flt_digit(ubel_big *x, const ubel_big *s1, const ubel_big *s2, const ubel_big *s4, const ubel_big *s8) {
    int d = 0;
    if (ubel_big_cmp(x, s8) >= 0) { ubel_big_sub(x, s8); d += 8; }
    if (ubel_big_cmp(x, s4) >= 0) { ubel_big_sub(x, s4); d += 4; }
    if (ubel_big_cmp(x, s2) >= 0) { ubel_big_sub(x, s2); d += 2; }
    if (ubel_big_cmp(x, s1) >= 0) { ubel_big_sub(x, s1); d += 1; }
    return d;
}

/* `flt2dec::round_up`: increase the last digit and propagate the carry; returns 1 when 99..9 became 100..0 and the
 * caller must append a '0' and raise the exponent. */
static inline int ubel_flt_round_up(char *d, int n) {
    int i = n - 1;
    while (i >= 0 && d[i] == '9') i--;
    if (i >= 0) {
        d[i]++;
        for (int j = i + 1; j < n; j++) d[j] = '0';
        return 0;
    }
    d[0] = '1';
    for (int j = 1; j < n; j++) d[j] = '0';
    return 1;
}

/* `dragon::format_shortest`: writes the digits (at least 1, at most 17) to buf, returns how many, and stores
 * the decimal exponent k so that the value is 0.DIGITS * 10^k. */
static int ubel_flt_shortest(const ubel_decoded *dd, char *buf, int *k_out) {
    int k = ubel_flt_estimate_k(dd->mant + dd->plus, dd->exp);
    int inc = dd->inclusive;
    ubel_big mant, minus, plus, scale, tmp;
    ubel_big_from_u64(&mant, dd->mant);
    ubel_big_from_u64(&minus, dd->minus);
    ubel_big_from_u64(&plus, dd->plus);
    ubel_big_from_u64(&scale, 1);
    scale.n = 1;
    if (dd->exp < 0) {
        ubel_big_mul_pow2(&scale, -dd->exp);
    } else {
        ubel_big_mul_pow2(&mant, dd->exp);
        ubel_big_mul_pow2(&minus, dd->exp);
        ubel_big_mul_pow2(&plus, dd->exp);
    }
    if (k >= 0) {
        ubel_big_mul_pow10(&scale, k);
    } else {
        ubel_big_mul_pow10(&mant, -k);
        ubel_big_mul_pow10(&minus, -k);
        ubel_big_mul_pow10(&plus, -k);
    }
    tmp = mant;
    ubel_big_add(&tmp, &plus);
    if (ubel_flt_lt(ubel_big_cmp(&scale, &tmp), inc)) {
        k += 1;
    } else {
        ubel_big_mul_small(&mant, 10);
        ubel_big_mul_small(&minus, 10);
        ubel_big_mul_small(&plus, 10);
    }
    ubel_big scale2 = scale, scale4 = scale, scale8 = scale;
    ubel_big_mul_pow2(&scale2, 1);
    ubel_big_mul_pow2(&scale4, 2);
    ubel_big_mul_pow2(&scale8, 3);

    int down, up, i = 0;
    for (;;) {
        int d = ubel_flt_digit(&mant, &scale, &scale2, &scale4, &scale8);
        buf[i++] = (char)('0' + d);
        down = ubel_flt_lt(ubel_big_cmp(&mant, &minus), inc);
        tmp = mant;
        ubel_big_add(&tmp, &plus);
        up = ubel_flt_lt(ubel_big_cmp(&scale, &tmp), inc);
        if (down || up) break;
        ubel_big_mul_small(&mant, 10);
        ubel_big_mul_small(&minus, 10);
        ubel_big_mul_small(&plus, 10);
    }
    if (up) {
        int round = !down;
        if (!round) {
            ubel_big_mul_pow2(&mant, 1);
            round = ubel_big_cmp(&mant, &scale) >= 0;
        }
        if (round && ubel_flt_round_up(buf, i)) {
            buf[i++] = '0';
            k += 1;
        }
    }
    *k_out = k;
    return i;
}

/* Digits to text, `flt2dec::digits_to_dec_str` with no fractional digits asked for: no exponent, no ".0". */
static int64_t ubel_flt_layout(char *out, const char *digits, int n, int exp) {
    char *p = out;
    if (exp <= 0) {
        *p++ = '0';
        *p++ = '.';
        for (int i = 0; i < -exp; i++) *p++ = '0';
        memcpy(p, digits, (size_t)n);
        p += n;
    } else if (exp < n) {
        memcpy(p, digits, (size_t)exp);
        p += exp;
        *p++ = '.';
        memcpy(p, digits + exp, (size_t)(n - exp));
        p += n - exp;
    } else {
        memcpy(p, digits, (size_t)n);
        p += n;
        for (int i = 0; i < exp - n; i++) *p++ = '0';
    }
    return p - out;
}

/* Shared tail: `cat` is 0 NaN, 1 infinite, 2 zero, 3 finite. */
static int64_t ubel_flt_emit(char *out, int neg, int cat, const ubel_decoded *dd) {
    char *p = out;
    if (cat == 0) { memcpy(p, "NaN", 3); return 3; }
    if (neg) *p++ = '-';
    if (cat == 1) { memcpy(p, "inf", 3); return (p - out) + 3; }
    if (cat == 2) { *p++ = '0'; return p - out; }
    char digits[24];
    int k;
    int n = ubel_flt_shortest(dd, digits, &k);
    p += ubel_flt_layout(p, digits, n, k);
    return p - out;
}

/* `{}` of a double, written to out (UBEL_FLT_BUF bytes at least), no terminator; returns the length. */
static int64_t ubel_f64_format(char *out, double v) {
    uint64_t bits;
    memcpy(&bits, &v, sizeof bits);
    int neg = (int)(bits >> 63);
    int e = (int)((bits >> 52) & 0x7ff);
    uint64_t frac = bits & 0xfffffffffffffULL;
    ubel_decoded dd;
    memset(&dd, 0, sizeof dd);
    if (e == 0x7ff) return ubel_flt_emit(out, neg, frac ? 0 : 1, &dd);
    if (e == 0 && frac == 0) return ubel_flt_emit(out, neg, 2, &dd);
    uint64_t mant = e == 0 ? frac << 1 : (frac | 0x10000000000000ULL);
    int exp = e - (1023 + 52);
    dd.inclusive = (mant & 1) == 0;
    if (e == 0) {                                     /* subnormal */
        dd.mant = mant; dd.minus = 1; dd.plus = 1; dd.exp = exp;
    } else if (mant == 0x10000000000000ULL) {         /* a power of two: the gap below is half the gap above */
        dd.mant = mant << 2; dd.minus = 1; dd.plus = 2; dd.exp = exp - 2;
    } else {
        dd.mant = mant << 1; dd.minus = 1; dd.plus = 1; dd.exp = exp - 1;
    }
    return ubel_flt_emit(out, neg, 3, &dd);
}

/* `{}` of a float (f32): the shortest digits for the 24-bit mantissa, not for the double it widens to. */
static int64_t ubel_f32_format(char *out, float v) {
    uint32_t bits;
    memcpy(&bits, &v, sizeof bits);
    int neg = (int)(bits >> 31);
    int e = (int)((bits >> 23) & 0xff);
    uint32_t frac = bits & 0x7fffffu;
    ubel_decoded dd;
    memset(&dd, 0, sizeof dd);
    if (e == 0xff) return ubel_flt_emit(out, neg, frac ? 0 : 1, &dd);
    if (e == 0 && frac == 0) return ubel_flt_emit(out, neg, 2, &dd);
    uint64_t mant = e == 0 ? (uint64_t)frac << 1 : (uint64_t)(frac | 0x800000u);
    int exp = e - (127 + 23);
    dd.inclusive = (mant & 1) == 0;
    if (e == 0) {
        dd.mant = mant; dd.minus = 1; dd.plus = 1; dd.exp = exp;
    } else if (mant == 0x800000u) {
        dd.mant = mant << 2; dd.minus = 1; dd.plus = 2; dd.exp = exp - 2;
    } else {
        dd.mant = mant << 1; dd.minus = 1; dd.plus = 1; dd.exp = exp - 1;
    }
    return ubel_flt_emit(out, neg, 3, &dd);
}

#endif /* UBEL_FLOAT_H */
