//! Small dense least squares: Gaussian elimination and Levenberg–Marquardt with a numeric Jacobian.

/// Solve `a · x = b` (n × n, row-major) by Gaussian elimination with partial pivoting; `None` when singular.
pub fn solve(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Option<Vec<f64>> {
    let n = b.len();
    if a.len() != n || a.iter().any(|r| r.len() != n) {
        return None;
    }
    for col in 0..n {
        let pivot = (col..n).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if a[pivot][col].abs() <= 1e-300 || a[pivot][col].is_nan() {
            return None;
        }
        a.swap(col, pivot);
        b.swap(col, pivot);
        for row in col + 1..n {
            let f = a[row][col] / a[col][col];
            if f != 0.0 {
                for k in col..n {
                    a[row][k] -= f * a[col][k];
                }
                b[row] -= f * b[col];
            }
        }
    }
    let mut x = vec![0.0; n];
    for row in (0..n).rev() {
        let s: f64 = (row + 1..n).map(|k| a[row][k] * x[k]).sum();
        x[row] = (b[row] - s) / a[row][row];
    }
    x.iter().all(|v| v.is_finite()).then_some(x)
}

fn cost(r: &[f64]) -> f64 {
    r.iter().map(|v| v * v).sum()
}

/// Minimise `Σ f(x)²` from `x0` by Levenberg–Marquardt (forward-difference Jacobian, step `h`), projecting every
/// accepted point with `project`. Returns the best point found (never worse than `x0`).
pub fn levenberg_marquardt(x0: Vec<f64>, iterations: usize, h: f64, f: impl Fn(&[f64]) -> Vec<f64>, project: impl Fn(&mut [f64])) -> Vec<f64> {
    let n = x0.len();
    let mut x = x0;
    project(&mut x);
    let mut r = f(&x);
    let mut c = cost(&r);
    if !c.is_finite() {
        return x;
    }
    let mut mu = 1e-3;
    for _ in 0..iterations {
        // Jacobian columns
        let mut jac: Vec<Vec<f64>> = Vec::with_capacity(n);
        for i in 0..n {
            let mut xp = x.clone();
            xp[i] += h;
            let rp = f(&xp);
            if rp.len() != r.len() {
                return x;
            }
            jac.push(rp.iter().zip(&r).map(|(a, b)| (a - b) / h).collect());
        }
        let jtj: Vec<Vec<f64>> = (0..n).map(|i| (0..n).map(|j| jac[i].iter().zip(&jac[j]).map(|(a, b)| a * b).sum()).collect()).collect();
        let jtr: Vec<f64> = (0..n).map(|i| jac[i].iter().zip(&r).map(|(a, b)| a * b).sum()).collect();
        let mut improved = false;
        for _ in 0..8 {
            let mut a = jtj.clone();
            for (i, row) in a.iter_mut().enumerate() {
                row[i] += mu * (jtj[i][i] + 1e-9);
            }
            let Some(step) = solve(a, jtr.iter().map(|v| -v).collect()) else {
                mu *= 10.0;
                continue;
            };
            let mut xn: Vec<f64> = x.iter().zip(&step).map(|(a, b)| a + b).collect();
            project(&mut xn);
            let rn = f(&xn);
            let cn = cost(&rn);
            if cn.is_finite() && cn < c {
                x = xn;
                r = rn;
                c = cn;
                mu = (mu * 0.3).max(1e-9);
                improved = true;
                break;
            }
            mu *= 10.0;
        }
        if !improved {
            break;
        }
    }
    x
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_linalg_solves_and_minimises() {
        let x = solve(vec![vec![2.0, 1.0], vec![1.0, 3.0]], vec![3.0, 5.0]).unwrap();
        assert!((x[0] - 0.8).abs() < 1e-12 && (x[1] - 1.4).abs() < 1e-12);
        assert!(solve(vec![vec![1.0, 2.0], vec![2.0, 4.0]], vec![1.0, 1.0]).is_none());
        // Rosenbrock-like
        let best = levenberg_marquardt(vec![-1.0, 1.0], 100, 1e-7, |p| vec![10.0 * (p[1] - p[0] * p[0]), 1.0 - p[0]], |_| {});
        assert!((best[0] - 1.0).abs() < 1e-3 && (best[1] - 1.0).abs() < 1e-3, "{best:?}");
    }
}
