// fannkuch-redux: see fannkuch.ubl. Same algorithm.
package main

import "fmt"

func main() {
	n := @N@
	perm1 := make([]int, n)
	perm := make([]int, n)
	count := make([]int, n)
	for i := 0; i < n; i++ {
		perm1[i] = i
	}
	maxFlips, permCount, checksum := 0, 0, 0
	r := n
	running := true
	for running {
		for r != 1 {
			count[r-1] = r
			r--
		}
		for i := 0; i < n; i++ {
			perm[i] = perm1[i]
		}
		flips := 0
		k := perm[0]
		for k != 0 {
			i, j := 0, k
			for i < j {
				perm[i], perm[j] = perm[j], perm[i]
				i++
				j--
			}
			flips++
			k = perm[0]
		}
		if flips > maxFlips {
			maxFlips = flips
		}
		if permCount%2 == 0 {
			checksum += flips
		} else {
			checksum -= flips
		}
		more := true
		for more {
			if r == n {
				running = false
				more = false
			} else {
				p0 := perm1[0]
				i := 0
				for i < r {
					perm1[i] = perm1[i+1]
					i++
				}
				perm1[r] = p0
				count[r]--
				if count[r] > 0 {
					more = false
				} else {
					r++
				}
			}
		}
		permCount++
	}
	fmt.Println(checksum)
	fmt.Println(maxFlips)
}
