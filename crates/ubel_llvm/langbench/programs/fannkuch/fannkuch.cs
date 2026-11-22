// fannkuch-redux: see fannkuch.ubl. Same algorithm.
using System;

static class Program
{
    static void Main()
    {
        int n = @N@;
        int[] perm1 = new int[n], perm = new int[n], count = new int[n];
        for (int i = 0; i < n; i++) { perm1[i] = i; perm[i] = 0; count[i] = 0; }
        long maxFlips = 0, permCount = 0, checksum = 0;
        int r = n;
        bool running = true;
        while (running)
        {
            while (r != 1) { count[r - 1] = r; r -= 1; }
            for (int i = 0; i < n; i++) perm[i] = perm1[i];
            long flips = 0;
            int k = perm[0];
            while (k != 0)
            {
                int i = 0, j = k;
                while (i < j) { int t = perm[i]; perm[i] = perm[j]; perm[j] = t; i += 1; j -= 1; }
                flips += 1;
                k = perm[0];
            }
            if (flips > maxFlips) maxFlips = flips;
            if (permCount % 2 == 0) checksum += flips; else checksum -= flips;
            bool more = true;
            while (more)
            {
                if (r == n)
                {
                    running = false;
                    more = false;
                }
                else
                {
                    int p0 = perm1[0];
                    int i = 0;
                    while (i < r) { perm1[i] = perm1[i + 1]; i += 1; }
                    perm1[r] = p0;
                    count[r] -= 1;
                    if (count[r] > 0) more = false; else r += 1;
                }
            }
            permCount += 1;
        }
        Console.WriteLine(checksum);
        Console.WriteLine(maxFlips);
    }
}
