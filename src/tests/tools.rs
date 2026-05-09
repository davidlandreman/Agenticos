//! Tests for the tool registry (`src/tools/mod.rs`) and downstream tool
//! implementations. Exercises the registry directly without going through the
//! serial transport — that's the in-kernel consumer R16 promises.

use alloc::string::{String, ToString};
use alloc::boxed::Box;
use crate::lib::test_utils::Testable;
use crate::tools::{Tool, ToolError, ToolRegistry, ToolResult};
use crate::debug_debug;

struct FakeOk;

impl Tool for FakeOk {
    fn name(&self) -> &'static str { "fake_ok" }
    fn description(&self) -> &'static str { "always-succeeds test tool" }
    fn schema(&self) -> &'static str { "{}" }
    fn call(&self, _args_json: &str) -> Result<ToolResult, ToolError> {
        Ok(ToolResult::json_only(String::from("{\"hello\":\"world\"}")))
    }
}

struct FakeFail;

impl Tool for FakeFail {
    fn name(&self) -> &'static str { "fake_fail" }
    fn description(&self) -> &'static str { "always-fails test tool" }
    fn schema(&self) -> &'static str { "{}" }
    fn call(&self, _args_json: &str) -> Result<ToolResult, ToolError> {
        Err(ToolError::tool_failed("intentional"))
    }
}

struct FakeBinary;

impl Tool for FakeBinary {
    fn name(&self) -> &'static str { "fake_binary" }
    fn description(&self) -> &'static str { "returns a binary trailer" }
    fn schema(&self) -> &'static str { "{}" }
    fn call(&self, _args_json: &str) -> Result<ToolResult, ToolError> {
        Ok(ToolResult::with_binary(
            String::from("{\"len\":3}"),
            alloc::vec![1u8, 2, 3],
        ))
    }
}

fn test_registry_happy_path() {
    debug_debug!("Testing registry happy path...");
    let mut reg = ToolRegistry::new();
    reg.register(Box::new(FakeOk));
    let result = reg.call("fake_ok", "{}").expect("call should succeed");
    assert_eq!(result.json, "{\"hello\":\"world\"}");
    assert!(result.binary.is_none());
    debug_debug!("Registry happy path passed!");
}

fn test_registry_unknown_tool() {
    debug_debug!("Testing unknown_tool error...");
    let reg = ToolRegistry::new();
    let err = reg.call("nonexistent", "{}").expect_err("should be unknown");
    assert_eq!(err.code, "unknown_tool");
    debug_debug!("Unknown tool error passed!");
}

fn test_registry_propagates_tool_error() {
    debug_debug!("Testing tool error propagation...");
    let mut reg = ToolRegistry::new();
    reg.register(Box::new(FakeFail));
    let err = reg.call("fake_fail", "{}").expect_err("should propagate");
    assert_eq!(err.code, "tool_failed");
    assert_eq!(err.message, "intentional");
    debug_debug!("Tool error propagation passed!");
}

fn test_registry_binary_trailer() {
    debug_debug!("Testing binary trailer plumbing...");
    let mut reg = ToolRegistry::new();
    reg.register(Box::new(FakeBinary));
    let result = reg.call("fake_binary", "{}").expect("call should succeed");
    assert_eq!(result.json, "{\"len\":3}");
    assert_eq!(result.binary.as_deref(), Some(&[1u8, 2, 3][..]));
    debug_debug!("Binary trailer plumbing passed!");
}

fn test_registry_enumerate_stable_order() {
    debug_debug!("Testing enumeration ordering...");
    let mut reg = ToolRegistry::new();
    // Register in reverse alphabetical order; BTreeMap should sort.
    reg.register(Box::new(FakeOk));
    reg.register(Box::new(FakeFail));
    reg.register(Box::new(FakeBinary));
    let names: alloc::vec::Vec<&'static str> = reg.enumerate().iter().map(|d| d.name).collect();
    assert_eq!(names, alloc::vec!["fake_binary", "fake_fail", "fake_ok"]);
    debug_debug!("Enumeration order passed!");
}

fn test_registry_duplicate_registration_latest_wins() {
    debug_debug!("Testing duplicate registration (latest-wins)...");
    let mut reg = ToolRegistry::new();
    reg.register(Box::new(FakeOk));

    struct ShadowOk;
    impl Tool for ShadowOk {
        fn name(&self) -> &'static str { "fake_ok" }
        fn description(&self) -> &'static str { "shadow" }
        fn schema(&self) -> &'static str { "{}" }
        fn call(&self, _args_json: &str) -> Result<ToolResult, ToolError> {
            Ok(ToolResult::json_only(String::from("{\"shadowed\":true}")))
        }
    }
    reg.register(Box::new(ShadowOk));
    let result = reg.call("fake_ok", "{}").expect("should succeed");
    assert_eq!(result.json, "{\"shadowed\":true}");
    debug_debug!("Latest-wins duplicate registration passed!");
}

fn test_tool_error_helpers() {
    debug_debug!("Testing ToolError helpers...");
    assert_eq!(ToolError::unknown_tool("x").code, "unknown_tool");
    assert_eq!(ToolError::bad_args("y").code, "bad_args");
    assert_eq!(ToolError::tool_failed("z").code, "tool_failed");
    assert_eq!(ToolError::unsupported("w").code, "unsupported");
    assert_eq!(ToolError::bad_args("y").message, "y".to_string());
    debug_debug!("ToolError helpers passed!");
}

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_registry_happy_path,
        &test_registry_unknown_tool,
        &test_registry_propagates_tool_error,
        &test_registry_binary_trailer,
        &test_registry_enumerate_stable_order,
        &test_registry_duplicate_registration_latest_wins,
        &test_tool_error_helpers,
    ]
}
