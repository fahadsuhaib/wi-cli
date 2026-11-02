#!/usr/bin/env python3
"""Split the terminal scripts into balanced CI shards.

usage: dev/shard.py <k> <n> [--local-only]

Prints the tasks ("<script> <local|remote>") that shard k of n should run, one per line. Tasks are
the local run and, for scripts marked `# remote: yes`, the remote run. They are assigned
longest-first to the least-loaded shard, using the measured seconds in dev/shard-weights.txt
(unknown tasks get DEFAULT seconds), so every shard finishes at about the same time.
"""
import glob, os, sys

DEFAULT = 40
here = os.path.dirname(os.path.abspath(__file__))


def tasks(local_only):
    out = []
    for path in sorted(glob.glob(os.path.join(here, "pty_*.py"))):
        name = "dev/" + os.path.basename(path)
        if name == "dev/pty_drive.py":  # the interactive driver, not a test
            continue
        out.append((name, "local"))
        if not local_only and "# remote: yes" in open(path, errors="replace").read():
            out.append((name, "remote"))
    return out


def weights():
    w = {}
    p = os.path.join(here, "shard-weights.txt")
    if os.path.exists(p):
        for line in open(p):
            parts = line.split()
            if len(parts) == 3 and not line.startswith("#"):
                w[(parts[0], parts[1])] = float(parts[2])
    return w


def assign(all_tasks, n, w):
    load = [0.0] * n
    shards = [[] for _ in range(n)]
    for t in sorted(all_tasks, key=lambda t: (-w.get(t, DEFAULT), t)):
        i = min(range(n), key=lambda j: (load[j], j))
        shards[i].append(t)
        load[i] += w.get(t, DEFAULT)
    return shards, load


if __name__ == "__main__":
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    k, n = int(args[0]), int(args[1])
    if not 1 <= k <= n:
        sys.exit("shard index must be 1..n")
    all_tasks = tasks("--local-only" in sys.argv)
    shards, load = assign(all_tasks, n, weights())
    order = {t: i for i, t in enumerate(all_tasks)}
    for t in sorted(shards[k - 1], key=order.get):
        print(*t)
    if "--summary" in sys.argv:
        print("# estimated seconds per shard:", [round(x) for x in load], file=sys.stderr)
