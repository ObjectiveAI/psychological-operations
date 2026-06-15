//! Publish several psyops, then confirm `psyops list` reports all of
//! them. (Old: psyops_list_multiple_psyops — was preloaded; now published
//! via the executor.)

use psychological_operations_tests::{Plugin, query_psyop};

#[tokio::test]
async fn psyops_list_multiple_psyops() {
    let p = Plugin::new("psyops_list_multiple_psyops");

    for name in ["a-psyop", "b-psyop", "c-psyop"] {
        p.psyops_publish(name, &query_psyop("mock fallback search", vec![]))
            .await
            .assert_no_errors();
    }

    let listed = p.psyops_list().await;
    listed.assert_no_errors();
    let names: Vec<&str> = listed.psyop_list().iter().map(|e| e.name.as_str()).collect();
    for expected in ["a-psyop", "b-psyop", "c-psyop"] {
        assert!(names.contains(&expected), "expected {expected} in {names:?}");
    }
}
