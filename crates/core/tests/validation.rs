//! Runs the analytic validation suite; every check must pass.

use exhaust_core::validation::cases;

#[test]
fn validation_suite_passes() {
    let mut failed = Vec::new();
    for (name, run) in cases() {
        let checks = run().unwrap_or_else(|e| panic!("case {name} failed to run: {e}"));
        for c in checks {
            println!(
                "{:<28} {:<48} {:>12.4e} <= {:<10.3e} {}",
                c.case,
                c.metric,
                c.value,
                c.limit,
                if c.pass { "ok" } else { "FAIL" }
            );
            if !c.pass {
                failed.push(format!("{}: {}", c.case, c.metric));
            }
        }
    }
    assert!(failed.is_empty(), "failed checks: {failed:?}");
}
