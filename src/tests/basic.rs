use crate::{debug_debug};
use crate::lib::test_utils::Testable;

fn test_debug_system() {
    debug_debug!("Debug system is working correctly");
}

fn test_basic_arithmetic() {
    let a = 10;
    let b = 20;
    assert_eq!(a + b, 30, "10 + 20 should equal 30");
    assert_eq!(b - a, 10, "20 - 10 should equal 10");
}

fn test_boolean_logic() {
    assert!(true, "true should be true");
    assert!(!false, "false should be false");
    assert_eq!(true && true, true);
    assert_eq!(true || false, true);
}

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_debug_system,
        &test_basic_arithmetic,
        &test_boolean_logic,
    ]
}