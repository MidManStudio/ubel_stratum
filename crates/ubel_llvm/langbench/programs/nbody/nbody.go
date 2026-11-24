// n-body: see nbody.ubl. Same algorithm and operation order. Bodies are pointers into a slice (references, like Ubel structs).
package main

import (
	"fmt"
	"math"
)

type Body struct {
	x, y, z, vx, vy, vz, mass float64
}

const pi = 3.141592653589793
const solarMass = 4.0 * pi * pi
const daysPerYear = 365.24

func newBody(x, y, z, vx, vy, vz, mass float64) *Body {
	return &Body{x, y, z, vx * daysPerYear, vy * daysPerYear, vz * daysPerYear, mass * solarMass}
}

func advance(b []*Body, dt float64) {
	n := len(b)
	for i := 0; i < n; i++ {
		bi := b[i]
		for j := i + 1; j < n; j++ {
			bj := b[j]
			dx := bi.x - bj.x
			dy := bi.y - bj.y
			dz := bi.z - bj.z
			d2 := dx*dx + dy*dy + dz*dz
			mag := dt / (d2 * math.Sqrt(d2))
			bi.vx -= dx * bj.mass * mag
			bi.vy -= dy * bj.mass * mag
			bi.vz -= dz * bj.mass * mag
			bj.vx += dx * bi.mass * mag
			bj.vy += dy * bi.mass * mag
			bj.vz += dz * bi.mass * mag
		}
	}
	for i := 0; i < n; i++ {
		bb := b[i]
		bb.x += dt * bb.vx
		bb.y += dt * bb.vy
		bb.z += dt * bb.vz
	}
}

func energy(b []*Body) float64 {
	n := len(b)
	e := 0.0
	for i := 0; i < n; i++ {
		bi := b[i]
		e += 0.5 * bi.mass * (bi.vx*bi.vx + bi.vy*bi.vy + bi.vz*bi.vz)
		for j := i + 1; j < n; j++ {
			bj := b[j]
			dx := bi.x - bj.x
			dy := bi.y - bj.y
			dz := bi.z - bj.z
			e -= bi.mass * bj.mass / math.Sqrt(dx*dx+dy*dy+dz*dz)
		}
	}
	return e
}

func main() {
	bodies := []*Body{
		{0.0, 0.0, 0.0, 0.0, 0.0, 0.0, solarMass},
		newBody(4.84143144246472090e+00, -1.16032004402742839e+00, -1.03622044471123109e-01, 1.66007664274403694e-03, 7.69901118419740425e-03, -6.90460016972063023e-05, 9.54791938424326609e-04),
		newBody(8.34336671824457987e+00, 4.12479856412430479e+00, -4.03523417114321381e-01, -2.76742510726862411e-03, 4.99852801234917238e-03, 2.30417297573763929e-05, 2.85885980666130812e-04),
		newBody(1.28943695621391310e+01, -1.51111514016986312e+01, -2.23307578892655734e-01, 2.96460137564761618e-03, 2.37847173959480950e-03, -2.96589568540237556e-05, 4.36624404335156298e-05),
		newBody(1.53796971148509165e+01, -2.59193146099879641e+01, 1.79258772950371181e-01, 2.68067772490389322e-03, 1.62824170038242295e-03, -9.51592254519715870e-05, 5.15138902046611451e-05),
	}
	px, py, pz := 0.0, 0.0, 0.0
	for _, b := range bodies {
		px += b.vx * b.mass
		py += b.vy * b.mass
		pz += b.vz * b.mass
	}
	bodies[0].vx = 0.0 - px/solarMass
	bodies[0].vy = 0.0 - py/solarMass
	bodies[0].vz = 0.0 - pz/solarMass
	fmt.Println(int64(energy(bodies) * 1000000000.0))
	steps := int64(@N@)
	for s := int64(0); s < steps; s++ {
		advance(bodies, 0.01)
	}
	fmt.Println(int64(energy(bodies) * 1000000000.0))
}
