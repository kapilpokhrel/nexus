#!/usr/bin/env python3

import pandas as pd
import matplotlib.pyplot as plt

plt.style.use("seaborn-v0_8-whitegrid")

df = pd.read_csv("results/results.csv")

fig, (ax1, ax2) = plt.subplots(
    2,
    1,
    figsize=(12, 9),
    sharex=True,
    gridspec_kw={"height_ratios": [1, 2]},
)

ax1.plot(
    df["concurrency"],
    df["rps"],
    marker="s",
    linewidth=3,
    color="black",
)

ax1.set_ylabel("Requests/sec")
ax1.set_title("wrk Benchmark Results")
ax1.grid(alpha=0.3)

for x, y in zip(df["concurrency"], df["rps"]):
    ax1.annotate(
        f"{int(y)}",
        (x, y),
        xytext=(0, 8),
        textcoords="offset points",
        ha="center",
        fontsize=8,
    )


latency_cols = [
    ("mean_ms", "Mean", "tab:blue"),
    ("p50_ms", "P50", "tab:green"),
    ("p75_ms", "P75", "tab:olive"),
    ("p90_ms", "P90", "tab:orange"),
    ("p99_ms", "P99", "tab:red"),
]

for col, label, color in latency_cols:
    ax2.plot(
        df["concurrency"],
        df[col],
        marker="s",
        linewidth=2,
        color=color,
        label=label,
    )

for metric in ["p90_ms", "p99_ms"]:
    for x, y in zip(df["concurrency"], df[metric]):
        ax2.annotate(
            f"{int(y)}",
            (x, y),
            xytext=(0, 8),
            textcoords="offset points",
            ha="center",
            fontsize=8,
        )

ax2.set_xlabel("Concurrency")
ax2.set_ylabel("Latency (ms)")

# useful because p99 grows much faster
ax2.set_yscale("log")

ax2.set_xticks(df["concurrency"])
ax2.grid(alpha=0.3)

ax2.legend(
    loc="upper left",
    ncol=3,
)


plt.tight_layout()

plt.savefig(
    "results/benchmark.png",
    dpi=300,
    bbox_inches="tight",
)

plt.show()

print("\nSaved: results/benchmark.png")
