// n-body: see nbody.ubl. Same algorithm and operation order. Bodies are class instances (references), like the structs in Ubel.
using System;

class Body
{
    public double x, y, z, vx, vy, vz, mass;
}

static class Program
{
    const double PI = 3.141592653589793;
    const double SOLAR_MASS = 4.0 * PI * PI;
    const double DAYS_PER_YEAR = 365.24;

    static Body Make(double x, double y, double z, double vx, double vy, double vz, double mass)
    {
        return new Body { x = x, y = y, z = z, vx = vx * DAYS_PER_YEAR, vy = vy * DAYS_PER_YEAR, vz = vz * DAYS_PER_YEAR, mass = mass * SOLAR_MASS };
    }

    static void Advance(Body[] b, double dt)
    {
        int n = b.Length;
        for (int i = 0; i < n; i++)
        {
            Body bi = b[i];
            for (int j = i + 1; j < n; j++)
            {
                Body bj = b[j];
                double dx = bi.x - bj.x;
                double dy = bi.y - bj.y;
                double dz = bi.z - bj.z;
                double d2 = dx * dx + dy * dy + dz * dz;
                double mag = dt / (d2 * Math.Sqrt(d2));
                bi.vx -= dx * bj.mass * mag;
                bi.vy -= dy * bj.mass * mag;
                bi.vz -= dz * bj.mass * mag;
                bj.vx += dx * bi.mass * mag;
                bj.vy += dy * bi.mass * mag;
                bj.vz += dz * bi.mass * mag;
            }
        }
        for (int i = 0; i < n; i++)
        {
            Body bb = b[i];
            bb.x += dt * bb.vx;
            bb.y += dt * bb.vy;
            bb.z += dt * bb.vz;
        }
    }

    static double Energy(Body[] b)
    {
        int n = b.Length;
        double e = 0.0;
        for (int i = 0; i < n; i++)
        {
            Body bi = b[i];
            e += 0.5 * bi.mass * (bi.vx * bi.vx + bi.vy * bi.vy + bi.vz * bi.vz);
            for (int j = i + 1; j < n; j++)
            {
                Body bj = b[j];
                double dx = bi.x - bj.x;
                double dy = bi.y - bj.y;
                double dz = bi.z - bj.z;
                e -= bi.mass * bj.mass / Math.Sqrt(dx * dx + dy * dy + dz * dz);
            }
        }
        return e;
    }

    static void Main()
    {
        Body[] bodies = new Body[5];
        bodies[0] = new Body { x = 0.0, y = 0.0, z = 0.0, vx = 0.0, vy = 0.0, vz = 0.0, mass = SOLAR_MASS };
        bodies[1] = Make(4.84143144246472090e+00, -1.16032004402742839e+00, -1.03622044471123109e-01, 1.66007664274403694e-03, 7.69901118419740425e-03, -6.90460016972063023e-05, 9.54791938424326609e-04);
        bodies[2] = Make(8.34336671824457987e+00, 4.12479856412430479e+00, -4.03523417114321381e-01, -2.76742510726862411e-03, 4.99852801234917238e-03, 2.30417297573763929e-05, 2.85885980666130812e-04);
        bodies[3] = Make(1.28943695621391310e+01, -1.51111514016986312e+01, -2.23307578892655734e-01, 2.96460137564761618e-03, 2.37847173959480950e-03, -2.96589568540237556e-05, 4.36624404335156298e-05);
        bodies[4] = Make(1.53796971148509165e+01, -2.59193146099879641e+01, 1.79258772950371181e-01, 2.68067772490389322e-03, 1.62824170038242295e-03, -9.51592254519715870e-05, 5.15138902046611451e-05);
        double px = 0.0, py = 0.0, pz = 0.0;
        for (int i = 0; i < 5; i++)
        {
            px += bodies[i].vx * bodies[i].mass;
            py += bodies[i].vy * bodies[i].mass;
            pz += bodies[i].vz * bodies[i].mass;
        }
        bodies[0].vx = 0.0 - px / SOLAR_MASS;
        bodies[0].vy = 0.0 - py / SOLAR_MASS;
        bodies[0].vz = 0.0 - pz / SOLAR_MASS;
        Console.WriteLine((long)(Energy(bodies) * 1000000000.0));
        long steps = @N@;
        for (long s = 0; s < steps; s++) Advance(bodies, 0.01);
        Console.WriteLine((long)(Energy(bodies) * 1000000000.0));
    }
}
