// spectral-norm: see spectralnorm.ubl. Same algorithm and operation order.
using System;

static class Program
{
    static double A(long i, long j)
    {
        return 1.0 / (double)((i + j) * (i + j + 1) / 2 + i + 1);
    }

    static void MulAv(int n, double[] v, double[] av)
    {
        for (int i = 0; i < n; i++)
        {
            double s = 0.0;
            for (int j = 0; j < n; j++) s += A(i, j) * v[j];
            av[i] = s;
        }
    }

    static void MulAtv(int n, double[] v, double[] atv)
    {
        for (int i = 0; i < n; i++)
        {
            double s = 0.0;
            for (int j = 0; j < n; j++) s += A(j, i) * v[j];
            atv[i] = s;
        }
    }

    static void MulAtAv(int n, double[] v, double[] outv, double[] tmp)
    {
        MulAv(n, v, tmp);
        MulAtv(n, tmp, outv);
    }

    static void Main()
    {
        int n = @N@;
        double[] u = new double[n], v = new double[n], tmp = new double[n];
        for (int i = 0; i < n; i++) { u[i] = 1.0; v[i] = 0.0; tmp[i] = 0.0; }
        for (int r = 0; r < 10; r++)
        {
            MulAtAv(n, u, v, tmp);
            MulAtAv(n, v, u, tmp);
        }
        double vbv = 0.0, vv = 0.0;
        for (int i = 0; i < n; i++) { vbv += u[i] * v[i]; vv += v[i] * v[i]; }
        Console.WriteLine((long)(Math.Sqrt(vbv / vv) * 1000000000.0));
    }
}
