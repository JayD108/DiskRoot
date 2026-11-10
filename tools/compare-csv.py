"""Compare folder sizes in WizTree's CSV vs drishti's CSV (top 2 levels).

Usage: python tools/compare-csv.py wiztree.csv drishti.csv
"""
import csv
import sys

wt_path, dr_path = sys.argv[1], sys.argv[2]
MAX_DEPTH = 2  # C:\X = 1, C:\X\Y = 2


def depth(p: str) -> int:
    return p.rstrip("\\").count("\\")


def load_wiztree(path):
    out = {}
    with open(path, encoding="utf-8-sig", errors="replace", newline="") as f:
        next(f)  # banner line
        for row in csv.DictReader(f):
            p = row["File Name"]
            if p.endswith("\\") and depth(p) <= MAX_DEPTH:
                out[p.rstrip("\\").lower() or p.lower()] = (int(row["Size"]), int(row["Allocated"]), int(row["Files"] or 0))
    return out


def load_drishti(path):
    out = {}
    with open(path, encoding="utf-8", errors="replace", newline="") as f:
        for row in csv.DictReader(f):
            if row["Kind"] == "dir" and depth(row["Path"]) <= MAX_DEPTH:
                out[row["Path"].rstrip("\\").lower()] = (int(row["Size"]), int(row["OnDisk"]), int(row["Files"]))
    return out


wt, dr = load_wiztree(wt_path), load_drishti(dr_path)
gb = 1024 ** 3
rows = []
for p in set(wt) | set(dr):
    w, d = wt.get(p), dr.get(p)
    ws, ds = (w[0] if w else 0), (d[0] if d else 0)
    rows.append((abs(ws - ds), p, w, d))
rows.sort(reverse=True)

wroot, droot = wt.get("c:"), dr.get("c:")
print(f"Whole drive  WizTree: {wroot[0]/gb:,.2f} GB, {wroot[2]:,} files | drishti: {droot[0]/gb:,.2f} GB, {droot[2]:,} files")
print(f"Folders compared (top {MAX_DEPTH} levels): {len(rows)}")
exact = sum(1 for r in rows if r[2] and r[3] and r[2][0] == r[3][0] and r[2][2] == r[3][2])
within = sum(1 for r in rows if r[2] and r[3] and abs(r[2][0] - r[3][0]) <= max(1, 0.001 * r[2][0]))
print(f"Exact match (size + file count): {exact}/{len(rows)}   within 0.1%: {within}/{len(rows)}")
print("\nBiggest differences (logical size):")
print(f"{'WizTree':>12} {'drishti':>12} {'diff':>10}  files(wt/dr)        path")
for diff, p, w, d in rows[:15]:
    if diff == 0:
        break
    ws = w[0] / gb if w else float("nan")
    ds = d[0] / gb if d else float("nan")
    wf = w[2] if w else "-"
    df = d[2] if d else "-"
    print(f"{ws:11.2f}G {ds:11.2f}G {diff/gb:9.2f}G  {wf!s:>8}/{df!s:<8}  {p}")
