//! Tests for the Arc (Atomic Reference Counting) implementation

use crate::lib::arc::{Arc, Weak};
use crate::lib::test_utils::Testable;
use alloc::{vec, vec::Vec};
use alloc::string::String;

/// Test basic Arc creation and dereferencing
fn test_arc_new() {
    let arc = Arc::new(42);
    assert_eq!(*arc, 42);
    assert_eq!(Arc::strong_count(&arc), 1);
    assert_eq!(Arc::weak_count(&arc), 0);
}

/// Test Arc cloning increases reference count
fn test_arc_clone() {
    let arc1 = Arc::new(100);
    assert_eq!(Arc::strong_count(&arc1), 1);
    
    let arc2 = arc1.clone();
    assert_eq!(Arc::strong_count(&arc1), 2);
    assert_eq!(Arc::strong_count(&arc2), 2);
    assert_eq!(*arc1, 100);
    assert_eq!(*arc2, 100);
    
    let arc3 = arc2.clone();
    assert_eq!(Arc::strong_count(&arc1), 3);
    assert_eq!(Arc::strong_count(&arc2), 3);
    assert_eq!(Arc::strong_count(&arc3), 3);
}

/// Test Arc drop decreases reference count
fn test_arc_drop() {
    let arc1 = Arc::new(String::from("Hello"));
    let arc2 = arc1.clone();
    let arc3 = arc2.clone();
    
    assert_eq!(Arc::strong_count(&arc1), 3);
    
    drop(arc3);
    assert_eq!(Arc::strong_count(&arc1), 2);
    
    drop(arc2);
    assert_eq!(Arc::strong_count(&arc1), 1);
}

/// Test Arc with complex types
fn test_arc_complex_types() {
    // Test with Vec
    let vec_arc = Arc::new(vec![1, 2, 3, 4, 5]);
    let vec_clone = vec_arc.clone();
    assert_eq!(vec_arc[2], 3);
    assert_eq!(vec_clone.len(), 5);
    
    // Test with String
    let string_arc = Arc::new(String::from("AgenticOS"));
    let string_clone = string_arc.clone();
    assert_eq!(&*string_arc, "AgenticOS");
    assert_eq!(string_clone.len(), 9);
}

/// Test Arc::get_mut
fn test_arc_get_mut() {
    let mut arc = Arc::new(10);
    
    // Should get mutable reference when unique
    if let Some(val) = Arc::get_mut(&mut arc) {
        *val = 20;
    }
    assert_eq!(*arc, 20);
    
    // Should not get mutable reference when not unique
    let _arc2 = arc.clone();
    assert!(Arc::get_mut(&mut arc).is_none());
}

/// Test Arc::try_unwrap
fn test_arc_try_unwrap() {
    // Should succeed when unique
    let arc = Arc::new(String::from("unique"));
    match Arc::try_unwrap(arc) {
        Ok(val) => assert_eq!(val, "unique"),
        Err(_) => panic!("try_unwrap should succeed"),
    }
    
    // Should fail when not unique
    let arc1 = Arc::new(42);
    let _arc2 = arc1.clone();
    match Arc::try_unwrap(arc1) {
        Ok(_) => panic!("try_unwrap should fail"),
        Err(arc) => assert_eq!(*arc, 42),
    }
}

/// Test weak reference creation
fn test_weak_new() {
    let weak: Weak<i32> = Weak::new();
    assert_eq!(weak.strong_count(), 0);
    assert_eq!(weak.weak_count(), 0);
    assert!(weak.upgrade().is_none());
}

/// Test Arc::downgrade
fn test_arc_downgrade() {
    let arc = Arc::new(100);
    let weak = Arc::downgrade(&arc);
    
    assert_eq!(Arc::strong_count(&arc), 1);
    assert_eq!(Arc::weak_count(&arc), 1);
    assert_eq!(weak.strong_count(), 1);
    assert_eq!(weak.weak_count(), 1);
    
    // Upgrade should succeed while Arc exists
    if let Some(upgraded) = weak.upgrade() {
        assert_eq!(*upgraded, 100);
        assert_eq!(Arc::strong_count(&arc), 2);
    } else {
        panic!("Upgrade should succeed");
    }
}

/// Test weak reference behavior when Arc is dropped
fn test_weak_after_drop() {
    let arc = Arc::new(String::from("temporary"));
    let weak = Arc::downgrade(&arc);
    
    assert!(weak.upgrade().is_some());
    
    drop(arc);
    
    assert_eq!(weak.strong_count(), 0);
    assert!(weak.upgrade().is_none());
}

/// Test multiple weak references
fn test_multiple_weak_refs() {
    let arc = Arc::new(vec![1, 2, 3]);
    let weak1 = Arc::downgrade(&arc);
    let weak2 = Arc::downgrade(&arc);
    let weak3 = weak1.clone();
    
    assert_eq!(Arc::weak_count(&arc), 3);
    assert_eq!(weak1.weak_count(), 3);
    
    drop(weak3);
    assert_eq!(Arc::weak_count(&arc), 2);
    
    drop(arc);
    assert!(weak1.upgrade().is_none());
    assert!(weak2.upgrade().is_none());
}

/// Test Arc equality and ordering
fn test_arc_equality() {
    let arc1 = Arc::new(42);
    let arc2 = Arc::new(42);
    let arc3 = Arc::new(43);
    
    assert_eq!(arc1, arc2);
    assert_ne!(arc1, arc3);
    assert!(arc1 < arc3);
    assert!(arc3 > arc1);
}

/// Test Arc with zero-sized types
fn test_arc_zst() {
    struct ZeroSized;
    
    let arc1 = Arc::new(ZeroSized);
    let arc2 = arc1.clone();
    
    assert_eq!(Arc::strong_count(&arc1), 2);
    drop(arc2);
    assert_eq!(Arc::strong_count(&arc1), 1);
}

/// Test that Arc properly deallocates memory
fn test_arc_deallocation() {
    // Create and drop many Arcs to test memory management
    for _ in 0..100 {
        let arc = Arc::new(vec![0u8; 1024]); // 1KB allocation
        let _clone = arc.clone();
        // Both will be dropped at end of scope
    }
    
    // Create Arc with weak references
    for _ in 0..100 {
        let arc = Arc::new(String::from("test"));
        let _weak = Arc::downgrade(&arc);
        // Both will be dropped
    }
}

/// Get all Arc tests
pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_arc_new,
        &test_arc_clone,
        &test_arc_drop,
        &test_arc_complex_types,
        &test_arc_get_mut,
        &test_arc_try_unwrap,
        &test_weak_new,
        &test_arc_downgrade,
        &test_weak_after_drop,
        &test_multiple_weak_refs,
        &test_arc_equality,
        &test_arc_zst,
        &test_arc_deallocation,
    ]
}