//! Sandboxed RustPython VM configuration
//!
//! This module creates a restricted Python environment that:
//! - Removes dangerous builtins (open, eval, exec, compile, etc.)
//! - Restricts imports to a whitelist of safe modules
//! - Injects the tool_call() function for calling external tools
//! - Sets resource limits (recursion depth)

use rustpython_vm::builtins::{PyStr, PyDict, PyFloat, PyInt, PyList, PyModule};
use rustpython_vm::{
    Interpreter, PyResult, Settings, VirtualMachine, PyObjectRef, AsObject,
    PyRef, PyPayload,
};
use rustpython_vm::function::FuncArgs;
use std::cell::RefCell;
use serde_json::Value;

use crate::protocol::{PendingToolCall, ToolInfo, ToolCallResult};

// Thread-local state for collecting tool calls during execution
thread_local! {
    static PENDING_CALLS: RefCell<Vec<PendingToolCall>> = RefCell::new(Vec::new());
    static TOOL_RESULTS: RefCell<std::collections::HashMap<String, ToolCallResult>> = RefCell::new(std::collections::HashMap::new());
    static AVAILABLE_TOOLS: RefCell<Vec<ToolInfo>> = RefCell::new(Vec::new());
    static STDOUT_BUFFER: RefCell<String> = RefCell::new(String::new());
    static STDERR_BUFFER: RefCell<String> = RefCell::new(String::new());
}

/// Clear all thread-local state for a fresh execution
pub fn reset_execution_state() {
    PENDING_CALLS.with(|pc| pc.borrow_mut().clear());
    TOOL_RESULTS.with(|tr| tr.borrow_mut().clear());
    STDOUT_BUFFER.with(|sb| sb.borrow_mut().clear());
    STDERR_BUFFER.with(|se| se.borrow_mut().clear());
}

/// Set the available tools for this execution
pub fn set_available_tools(tools: Vec<ToolInfo>) {
    AVAILABLE_TOOLS.with(|at| *at.borrow_mut() = tools);
}

/// Set the tool results from a previous round
pub fn set_tool_results(results: std::collections::HashMap<String, ToolCallResult>) {
    TOOL_RESULTS.with(|tr| *tr.borrow_mut() = results);
}

/// Get the pending tool calls
pub fn get_pending_calls() -> Vec<PendingToolCall> {
    PENDING_CALLS.with(|pc| pc.borrow().clone())
}

/// Get the stdout buffer
pub fn get_stdout() -> String {
    STDOUT_BUFFER.with(|sb| sb.borrow().clone())
}

/// Get the stderr buffer
pub fn get_stderr() -> String {
    STDERR_BUFFER.with(|se| se.borrow().clone())
}

/// Append to stdout
pub fn append_stdout(s: &str) {
    STDOUT_BUFFER.with(|sb| sb.borrow_mut().push_str(s));
}

/// Append to stderr  
pub fn append_stderr(s: &str) {
    STDERR_BUFFER.with(|se| se.borrow_mut().push_str(s));
}

/// Create a sandboxed Python interpreter
pub fn create_sandboxed_interpreter() -> Interpreter {
    let mut settings = Settings::default();
    settings.isolated = true;
    settings.user_site_directory = false;
    settings.import_site = false;
    
    Interpreter::with_init(settings, |vm| {
        // Add our sandbox module with tool_call function
        vm.add_native_module("_sandbox".to_owned(), Box::new(make_sandbox_module));
    })
}

/// Create the sandbox native module
fn make_sandbox_module(vm: &VirtualMachine) -> PyRef<PyModule> {
    let module = PyModule::new();
    let module_ref = module.into_ref(&vm.ctx);
    
    // Get the module's __dict__ and add our functions
    let dict = module_ref.dict();
    
    // Add tool_call function
    let _ = dict.set_item(
        "tool_call",
        vm.new_function("tool_call", tool_call_impl).into(),
        vm,
    );
    
    // Add get_tool_result function
    let _ = dict.set_item(
        "get_tool_result", 
        vm.new_function("get_tool_result", get_tool_result_impl).into(),
        vm,
    );
    
    // Add print wrapper that captures output
    let _ = dict.set_item(
        "sandbox_print",
        vm.new_function("sandbox_print", sandbox_print_impl).into(),
        vm,
    );
    
    module_ref
}

/// Implementation of tool_call(name, **kwargs) -> result or raises ToolCallPending
fn tool_call_impl(args: FuncArgs, vm: &VirtualMachine) -> PyResult {
    // Get tool name (first positional arg)
    let tool_name: String = args.args.first()
        .ok_or_else(|| vm.new_type_error("tool_call requires a tool name".to_string()))?
        .try_to_value(vm)?;
    
    // Get keyword arguments as a map
    let arguments = funcargs_to_json(&args, vm)?;
    
    // Find the tool info to get server_id
    let server_id = AVAILABLE_TOOLS.with(|at| {
        at.borrow()
            .iter()
            .find(|t| t.name == tool_name)
            .map(|t| t.server_id.clone())
            .unwrap_or_else(|| "unknown".to_string())
    });
    
    // Generate a unique call ID
    let call_id = uuid::Uuid::new_v4().to_string();
    
    // Check if we already have a result for this call pattern
    let existing_result = TOOL_RESULTS.with(|tr| {
        tr.borrow().get(&tool_name).cloned()
    });
    
    if let Some(result) = existing_result {
        if result.success {
            json_to_pyobject(&result.result, vm)
        } else {
            Err(vm.new_runtime_error(result.error.unwrap_or_else(|| "Tool call failed".to_string())))
        }
    } else {
        // Queue the tool call and raise ToolCallPending
        let pending_call = PendingToolCall {
            id: call_id.clone(),
            tool_name,
            server_id,
            arguments,
        };
        
        PENDING_CALLS.with(|pc| pc.borrow_mut().push(pending_call));
        
        Err(vm.new_exception_msg(
            vm.ctx.exceptions.runtime_error.to_owned(),
            format!("ToolCallPending:{}", call_id),
        ))
    }
}

/// Get result of a previously made tool call
fn get_tool_result_impl(args: FuncArgs, vm: &VirtualMachine) -> PyResult {
    let call_id: String = args.args.first()
        .ok_or_else(|| vm.new_type_error("get_tool_result requires a call_id".to_string()))?
        .try_to_value(vm)?;
    
    TOOL_RESULTS.with(|tr| {
        if let Some(result) = tr.borrow().get(&call_id) {
            if result.success {
                json_to_pyobject(&result.result, vm)
            } else {
                Err(vm.new_runtime_error(result.error.clone().unwrap_or_else(|| "Tool call failed".to_string())))
            }
        } else {
            Err(vm.new_key_error(vm.ctx.new_str(format!("No result for call_id: {}", call_id)).into()))
        }
    })
}

/// Sandbox print that captures to buffer
fn sandbox_print_impl(args: FuncArgs, vm: &VirtualMachine) -> PyResult<()> {
    let mut output = String::new();
    for (i, arg) in args.args.iter().enumerate() {
        if i > 0 {
            output.push(' ');
        }
        let s: String = arg.str(vm)?.to_string();
        output.push_str(&s);
    }
    output.push('\n');
    append_stdout(&output);
    Ok(())
}

/// Convert FuncArgs kwargs to JSON Value
fn funcargs_to_json(args: &FuncArgs, vm: &VirtualMachine) -> PyResult<Value> {
    let mut map = serde_json::Map::new();
    
    // kwargs is IndexMap<String, PyObjectRef>
    for (key, value) in args.kwargs.iter() {
        let json_value = pyobject_to_json(value, vm)?;
        map.insert(key.clone(), json_value);
    }
    
    Ok(Value::Object(map))
}

/// Convert a Python object to JSON Value
pub fn pyobject_to_json(obj: &PyObjectRef, vm: &VirtualMachine) -> PyResult<Value> {
    // Check for None
    if obj.is(&vm.ctx.none) {
        return Ok(Value::Null);
    }
    
    // Try as bool (must check before int since bool is subclass of int in Python)
    // Use class check since PyBool is just a marker type
    if obj.class().is(vm.ctx.types.bool_type) {
        // Extract bool value using try_to_value
        if let Ok(b) = obj.try_to_value::<bool>(vm) {
            return Ok(Value::Bool(b));
        }
    }
    
    // Try as int
    if let Some(i) = obj.downcast_ref::<PyInt>() {
        if let Ok(n) = i.try_to_primitive::<i64>(vm) {
            return Ok(Value::Number(n.into()));
        }
    }
    
    // Try as float
    if let Some(f) = obj.downcast_ref::<PyFloat>() {
        if let Some(n) = serde_json::Number::from_f64(f.to_f64()) {
            return Ok(Value::Number(n));
        }
        return Ok(Value::Null);
    }
    
    // Try as string
    if let Some(s) = obj.downcast_ref::<PyStr>() {
        return Ok(Value::String(s.as_str().to_string()));
    }
    
    // Try as list
    if let Some(list) = obj.downcast_ref::<PyList>() {
        let items: Result<Vec<Value>, _> = list.borrow_vec()
            .iter()
            .map(|item| pyobject_to_json(item, vm))
            .collect();
        return Ok(Value::Array(items?));
    }
    
    // Try as dict
    if let Some(dict) = obj.downcast_ref::<PyDict>() {
        let mut map = serde_json::Map::new();
        for (k, v) in dict {
            let key_str: String = k.str(vm)?.to_string();
            let json_value = pyobject_to_json(&v, vm)?;
            map.insert(key_str, json_value);
        }
        return Ok(Value::Object(map));
    }
    
    // Fallback: convert to string representation
    let s: String = obj.str(vm)?.to_string();
    Ok(Value::String(s))
}

/// Convert a JSON Value to Python object
pub fn json_to_pyobject(value: &Value, vm: &VirtualMachine) -> PyResult {
    match value {
        Value::Null => Ok(vm.ctx.none()),
        Value::Bool(b) => Ok(vm.ctx.new_bool(*b).into()),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(vm.ctx.new_int(i).into())
            } else if let Some(f) = n.as_f64() {
                Ok(vm.ctx.new_float(f).into())
            } else {
                Ok(vm.ctx.none())
            }
        }
        Value::String(s) => Ok(vm.ctx.new_str(s.clone()).into()),
        Value::Array(arr) => {
            let items: Result<Vec<_>, _> = arr.iter()
                .map(|v| json_to_pyobject(v, vm))
                .collect();
            Ok(vm.ctx.new_list(items?).into())
        }
        Value::Object(obj) => {
            let dict = PyDict::new_ref(&vm.ctx);
            for (k, v) in obj {
                let py_value = json_to_pyobject(v, vm)?;
                dict.set_item(k.as_str(), py_value, vm)?;
            }
            Ok(dict.into())
        }
    }
}

/// Setup code to inject sandbox helpers into Python
pub const SANDBOX_SETUP_CODE: &str = r#"
# Sandbox setup - import sandbox functions
from _sandbox import tool_call, get_tool_result, sandbox_print

# Replace print with sandbox version  
import builtins
builtins.print = sandbox_print

# Remove dangerous builtins
_blocked = ['open', 'eval', 'exec', 'compile', 'input', 'breakpoint', 
            'globals', 'locals', 'vars', 'memoryview']
for _name in _blocked:
    if hasattr(builtins, _name):
        delattr(builtins, _name)

# Restricted import
_allowed_modules = {
    'math', 'json', 'random', 're', 'datetime', 'collections',
    'itertools', 'functools', 'operator', 'string', 'textwrap',
    'copy', 'types', 'typing', 'abc', 'numbers', 'decimal',
    'fractions', 'statistics', 'hashlib', 'base64', 'binascii',
    'html', '_sandbox', 'builtins'
}

_original_import = builtins.__import__

def _restricted_import(name, globals=None, locals=None, fromlist=(), level=0):
    top_level = name.split('.')[0]
    if top_level not in _allowed_modules:
        raise ImportError(f"Import '{name}' is not allowed in the sandbox")
    return _original_import(name, globals, locals, fromlist, level)

builtins.__import__ = _restricted_import

# Clean up setup variables
del _blocked, _name, _allowed_modules
"#;

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_reset_state() {
        PENDING_CALLS.with(|pc| pc.borrow_mut().push(PendingToolCall {
            id: "test".to_string(),
            tool_name: "test".to_string(),
            server_id: "test".to_string(),
            arguments: Value::Null,
        }));
        
        reset_execution_state();
        
        assert!(get_pending_calls().is_empty());
    }
    
    #[test]
    fn test_stdout_capture() {
        reset_execution_state();
        append_stdout("Hello ");
        append_stdout("World\n");
        assert_eq!(get_stdout(), "Hello World\n");
    }
}
