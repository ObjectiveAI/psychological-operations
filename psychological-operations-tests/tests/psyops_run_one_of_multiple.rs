//! Two psyops published; `psyops run --name target-psyop` runs only the
//! named one — the sibling never ingests. (Old: psyops_run_one_of_multiple.)

use psychological_operations_tests::{Plugin, query_psyop};

fn psyop_ran(run: &psychological_operations_tests::RunResult, name: &str) -> bool {
    run.events.iter().any(|e| {
        e.get("psyop").and_then(|v| v.as_str()) == Some(name)
    })
}

#[tokio::test]
async fn psyops_run_one_of_multiple() {
    let p = Plugin::new("psyops_run_one_of_multiple");
    p.psyops_publish("target-psyop", &query_psyop("mock fallback search", vec![]))
        .await
        .assert_no_errors();
    p.psyops_publish("other-psyop", &query_psyop("mock fallback search", vec![]))
        .await
        .assert_no_errors();

    let run = p.psyops_run(&["target-psyop"], Some(42)).await;
    run.assert_no_errors();
    assert!(psyop_ran(&run, "target-psyop"), "target-psyop should run: {:?}", run.events);
    assert!(!psyop_ran(&run, "other-psyop"), "sibling should NOT run: {:?}", run.events);
}
