// spectral-norm: see spectralnorm.ubl. Same algorithm and operation order.
package main

import (
	"fmt"
	"math"
)

func a(i, j int64) float64 {
	return 1.0 / float64((i+j)*(i+j+1)/2+i+1)
}

func mulAv(n int, v, av []float64) {
	for i := 0; i < n; i++ {
		s := 0.0
		for j := 0; j < n; j++ {
			s += a(int64(i), int64(j)) * v[j]
		}
		av[i] = s
	}
}

func mulAtv(n int, v, atv []float64) {
	for i := 0; i < n; i++ {
		s := 0.0
		for j := 0; j < n; j++ {
			s += a(int64(j), int64(i)) * v[j]
		}
		atv[i] = s
	}
}

func mulAtAv(n int, v, out, tmp []float64) {
	mulAv(n, v, tmp)
	mulAtv(n, tmp, out)
}

func main() {
	n := @N@
	u := make([]float64, n)
	v := make([]float64, n)
	tmp := make([]float64, n)
	for i := 0; i < n; i++ {
		u[i] = 1.0
	}
	for r := 0; r < 10; r++ {
		mulAtAv(n, u, v, tmp)
		mulAtAv(n, v, u, tmp)
	}
	vbv, vv := 0.0, 0.0
	for i := 0; i < n; i++ {
		vbv += u[i] * v[i]
		vv += v[i] * v[i]
	}
	fmt.Println(int64(math.Sqrt(vbv/vv) * 1000000000.0))
}
