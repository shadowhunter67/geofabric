"""Tiny non-negative least squares on relative error (no scipy): try every column subset, keep the best non-negative fit."""
import itertools
import numpy as np

def fit(X, y):
    X = np.asarray(X, float); y = np.asarray(y, float)
    w = 1.0 / y
    best = None
    n = X.shape[1]
    for k in range(1, n + 1):
        for cols in itertools.combinations(range(n), k):
            c, *_ = np.linalg.lstsq(X[:, cols] * w[:, None], y * w, rcond=None)
            if (c < -1e-12).any():
                continue
            full = np.zeros(n); full[list(cols)] = c
            err = float(np.sum(((X @ full - y) / y) ** 2))
            if best is None or err < best[0]:
                best = (err, full)
    return best[1]

def factors(X, y, c):
    p = np.asarray(X, float) @ c; y = np.asarray(y, float)
    f = np.maximum(p / y, y / p)
    return float(np.median(f)), float(np.percentile(f, 90)), float(f.max())
