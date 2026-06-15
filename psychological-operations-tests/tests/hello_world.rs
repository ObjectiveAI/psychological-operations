//! Scaffold integration test — proves the crate builds and `cargo test`
//! runs here. Real cross-crate suites will replace/join this.

#[test]
fn hello_world() {
    assert_eq!(psychological_operations_tests::hello(), "hello, world");
}
