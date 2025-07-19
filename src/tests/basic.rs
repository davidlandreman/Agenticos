use crate::{debug_debug, debug_info};
use crate::lib::test_utils::Testable;
use spin::Lazy;

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

// Test lazy static initialization
static TEST_LAZY_SIMPLE: Lazy<u32> = Lazy::new(|| {
    debug_info!("TEST_LAZY_SIMPLE: Initializing...");
    42
});

static TEST_LAZY_COMPLEX: Lazy<Option<&'static str>> = Lazy::new(|| {
    debug_info!("TEST_LAZY_COMPLEX: Starting initialization...");
    debug_info!("TEST_LAZY_COMPLEX: Performing some work...");
    let result = Some("Lazy static initialized successfully");
    debug_info!("TEST_LAZY_COMPLEX: Initialization complete");
    result
});

fn test_lazy_static_initialization() {
    debug_info!("Testing lazy static initialization...");
    
    // Test simple lazy static
    debug_info!("About to access TEST_LAZY_SIMPLE...");
    let value = *TEST_LAZY_SIMPLE;
    assert_eq!(value, 42, "Simple lazy static should return 42");
    debug_info!("TEST_LAZY_SIMPLE access successful, value: {}", value);
    
    // Test complex lazy static
    debug_info!("About to access TEST_LAZY_COMPLEX...");
    let complex_value = *TEST_LAZY_COMPLEX;
    assert!(complex_value.is_some(), "Complex lazy static should return Some");
    assert_eq!(complex_value.unwrap(), "Lazy static initialized successfully");
    debug_info!("TEST_LAZY_COMPLEX access successful");
    
    // Access them again to ensure they don't re-initialize
    debug_info!("Accessing lazy statics second time (should not re-initialize)...");
    let value2 = *TEST_LAZY_SIMPLE;
    let complex_value2 = *TEST_LAZY_COMPLEX;
    assert_eq!(value2, 42);
    assert_eq!(complex_value2.unwrap(), "Lazy static initialized successfully");
    debug_info!("Second access successful - lazy statics working correctly");
}

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_debug_system,
        &test_basic_arithmetic,
        &test_boolean_logic,
        &test_lazy_static_initialization,
    ]
}