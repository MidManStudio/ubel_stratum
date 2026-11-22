/* spectral-norm: see spectralnorm.ubl. Same algorithm and operation order. */
#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

static double a(int64_t i, int64_t j) {
    return 1.0 / (double)((i + j) * (i + j + 1) / 2 + i + 1);
}

static void mul_av(int64_t n, const double *v, double *av) {
    for (int64_t i = 0; i < n; i++) {
        double s = 0.0;
        for (int64_t j = 0; j < n; j++) s += a(i, j) * v[j];
        av[i] = s;
    }
}

static void mul_atv(int64_t n, const double *v, double *atv) {
    for (int64_t i = 0; i < n; i++) {
        double s = 0.0;
        for (int64_t j = 0; j < n; j++) s += a(j, i) * v[j];
        atv[i] = s;
    }
}

static void mul_atav(int64_t n, const double *v, double *out, double *tmp) {
    mul_av(n, v, tmp);
    mul_atv(n, tmp, out);
}

int main(void) {
    int64_t n = @N@;
    double *u = malloc(n * sizeof(double)), *v = malloc(n * sizeof(double)), *tmp = malloc(n * sizeof(double));
    for (int64_t i = 0; i < n; i++) { u[i] = 1.0; v[i] = 0.0; tmp[i] = 0.0; }
    for (int r = 0; r < 10; r++) {
        mul_atav(n, u, v, tmp);
        mul_atav(n, v, u, tmp);
    }
    double vbv = 0.0, vv = 0.0;
    for (int64_t i = 0; i < n; i++) { vbv += u[i] * v[i]; vv += v[i] * v[i]; }
    printf("%lld\n", (long long)(sqrt(vbv / vv) * 1000000000.0));
    free(u); free(v); free(tmp);
    return 0;
}
