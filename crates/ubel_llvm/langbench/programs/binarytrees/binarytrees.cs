// binary-trees: see binarytrees.ubl. Class nodes on the garbage-collected heap; null is the empty tree.
using System;

class Node
{
    public Node l, r;
}

static class Program
{
    static Node Make(long depth)
    {
        Node n = new Node();
        if (depth > 0)
        {
            n.l = Make(depth - 1);
            n.r = Make(depth - 1);
        }
        return n;
    }

    static long Check(Node t)
    {
        if (t == null) return 0;
        return 1 + Check(t.l) + Check(t.r);
    }

    static void Main()
    {
        long n = @N@;
        long minDepth = 4, maxDepth = minDepth + 2;
        if (n > maxDepth) maxDepth = n;
        long stretch = maxDepth + 1;
        Console.WriteLine("stretch tree of depth " + stretch + " check: " + Check(Make(stretch)));
        Node longLived = Make(maxDepth);
        for (long d = minDepth; d <= maxDepth; d += 2)
        {
            long iterations = 1;
            for (long k = 0; k < maxDepth - d + minDepth; k++) iterations *= 2;
            long total = 0;
            for (long i = 0; i < iterations; i++) total += Check(Make(d));
            Console.WriteLine(iterations + " trees of depth " + d + " check: " + total);
        }
        Console.WriteLine("long lived tree of depth " + maxDepth + " check: " + Check(longLived));
    }
}
