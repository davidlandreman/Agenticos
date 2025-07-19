use alloc::{vec, vec::Vec, string::String};
use crate::lib::test_utils::Testable;
use crate::{debug_info, debug_debug};

fn test_vec_allocation() {
    debug_debug!("Testing Vec allocation...");
    
    let mut vec = vec![1, 2, 3, 4, 5];
    assert_eq!(vec.len(), 5);
    
    vec.push(6);
    assert_eq!(vec.len(), 6);
    assert_eq!(vec[5], 6);
    
    for i in 7..100 {
        vec.push(i);
    }
    assert_eq!(vec.len(), 99);
    
    debug_debug!("Vec allocation test passed!");
}

fn test_string_allocation() {
    debug_debug!("Testing String allocation...");
    
    let mut string = String::from("Hello, ");
    string.push_str("heap!");
    assert_eq!(&string, "Hello, heap!");
    
    let string2 = String::from("Dynamic memory is working!");
    assert_eq!(string2.len(), 26);
    
    debug_debug!("String allocation test passed!");
}

fn test_large_allocation() {
    debug_debug!("Testing large allocation...");
    
    let size = 1024 * 1024; // 1 MB
    let vec: Vec<u8> = vec![42; size];
    assert_eq!(vec.len(), size);
    assert_eq!(vec[0], 42);
    assert_eq!(vec[size - 1], 42);
    
    debug_debug!("Large allocation test passed!");
}

fn test_multiple_allocations() {
    debug_debug!("Testing multiple allocations...");
    
    let mut vecs: Vec<Vec<i32>> = Vec::new();
    
    for i in 0..10 {
        let mut v = Vec::new();
        for j in 0..100 {
            v.push(i * 100 + j);
        }
        vecs.push(v);
    }
    
    assert_eq!(vecs.len(), 10);
    assert_eq!(vecs[0].len(), 100);
    assert_eq!(vecs[9][99], 999);
    
    debug_debug!("Multiple allocations test passed!");
}

fn test_allocation_and_deallocation() {
    debug_debug!("Testing allocation and deallocation...");
    
    for _ in 0..100 {
        let v: Vec<u8> = vec![0; 1024]; // 1 KB
        assert_eq!(v.len(), 1024);
        // v is dropped here, memory should be freed
    }
    
    // If we get here without running out of memory, deallocation is working
    debug_debug!("Allocation and deallocation test passed!");
}

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_vec_allocation,
        &test_string_allocation,
        &test_large_allocation,
        &test_multiple_allocations,
        &test_allocation_and_deallocation,
    ]
}