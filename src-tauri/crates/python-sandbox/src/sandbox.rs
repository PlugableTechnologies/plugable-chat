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

use crate::protocol::{PendingToolCall, ToolInfo, ToolCallResult, ToolModuleInfo};

// Thread-local state for collecting tool calls during execution
thread_local! {
    static PENDING_CALLS: RefCell<Vec<PendingToolCall>> = const { RefCell::new(Vec::new()) };
    static TOOL_RESULTS: RefCell<std::collections::HashMap<String, ToolCallResult>> = RefCell::new(std::collections::HashMap::new());
    static AVAILABLE_TOOLS: RefCell<Vec<ToolInfo>> = const { RefCell::new(Vec::new()) };
    static STDOUT_BUFFER: RefCell<String> = const { RefCell::new(String::new()) };
    static STDERR_BUFFER: RefCell<String> = const { RefCell::new(String::new()) };
    /// Tool modules that should be injected as importable Python modules
    static TOOL_MODULES: RefCell<Vec<ToolModuleInfo>> = const { RefCell::new(Vec::new()) };
}

/// Clear all thread-local state for a fresh execution
pub fn reset_execution_state() {
    PENDING_CALLS.with(|pc| pc.borrow_mut().clear());
    TOOL_RESULTS.with(|tr| tr.borrow_mut().clear());
    STDOUT_BUFFER.with(|sb| sb.borrow_mut().clear());
    STDERR_BUFFER.with(|se| se.borrow_mut().clear());
    // Note: We don't clear TOOL_MODULES here as they persist across executions
}

/// Set the available tools for this execution
pub fn set_available_tools(tools: Vec<ToolInfo>) {
    AVAILABLE_TOOLS.with(|at| *at.borrow_mut() = tools);
}

/// Set the tool modules that should be injected as importable Python modules
pub fn set_tool_modules(modules: Vec<ToolModuleInfo>) {
    TOOL_MODULES.with(|tm| *tm.borrow_mut() = modules);
}

/// Get the current tool modules
pub fn get_tool_modules() -> Vec<ToolModuleInfo> {
    TOOL_MODULES.with(|tm| tm.borrow().clone())
}

/// Generate Python code that creates callable tool functions
/// 
/// This code creates wrapper functions that call the sandbox's `tool_call` function
/// under the hood. Functions are injected directly into the global namespace so they
/// can be called directly without imports (e.g., `list_dataset_ids()` works).
/// 
/// For compatibility, we also create simple namespace classes that can be imported.
pub fn generate_tool_module_code() -> String {
    let modules = get_tool_modules();
    if modules.is_empty() {
        return String::new();
    }
    
    let mut code = String::new();
    code.push_str("\n# ============== Dynamic Tool Functions ==============\n");
    code.push_str("# Auto-generated wrapper functions for MCP tools\n\n");
    
    // Generate all wrapper functions first (in global namespace)
    for module in &modules {
        code.push_str(&format!("# Tools from: {} (server: {})\n", module.python_name, module.server_id));
        
        for func in &module.functions {
            // Generate a global wrapper function for each tool
            let func_code = generate_global_tool_function(&func.name, &func.description);
            code.push_str(&func_code);
        }
        code.push_str("\n");
    }
    
    // Create namespace classes for module-style imports (e.g., from bigquery import list_dataset_ids)
    code.push_str("# ============== Module Namespaces ==============\n");
    code.push_str("# Namespace classes for module-style imports\n\n");
    code.push_str("import sys\n\n");
    
    for module in &modules {
        code.push_str(&format!("class _{}:\n", module.python_name));
        code.push_str(&format!("    '''MCP tools from server: {}'''\n", module.server_id));
        for func in &module.functions {
            // Use staticmethod so accessing via instance doesn't add 'self' as first arg
            code.push_str(&format!("    {} = staticmethod({})\n", func.name, func.name));
        }
        code.push_str("\n");
        
        // Register in sys.modules (use class, not instance, to avoid method binding issues)
        code.push_str(&format!("sys.modules['{}'] = _{}\n\n", module.python_name, module.python_name));
        
        // Add to allowed imports
        code.push_str(&format!("_sandbox_allowed_modules.add('{}')\n\n", module.python_name));
    }
    
    code
}

/// Generate Python code for a global tool function wrapper
fn generate_global_tool_function(func_name: &str, description: &Option<String>) -> String {
    let docstring = description.as_ref()
        .map(|d| format!("\"\"\"{}\"\"\"", d))
        .unwrap_or_else(|| format!("\"\"\"Call the {} tool\"\"\"", func_name));
    
    format!(
        r#"def {func}(**kwargs):
    {docstring}
    from _sandbox import tool_call
    return tool_call("{func}", **kwargs)

"#,
        func = func_name,
        docstring = docstring
    )
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
        // Add stdlib native modules (math, json, random, hashlib, etc.)
        // These are the Rust implementations of Python stdlib modules
        for (name, init) in rustpython_stdlib::get_module_inits() {
            vm.add_native_module(name, init);
        }
        
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

/// The list of modules allowed in the Python sandbox.
/// This is exposed as a constant so it can be referenced in error messages.
pub const ALLOWED_MODULES: &[&str] = &[
    "math", "json", "random", "re", "datetime", "collections",
    "itertools", "functools", "operator", "string", "textwrap",
    "copy", "types", "typing", "abc", "numbers", "decimal",
    "fractions", "statistics", "hashlib", "base64", "binascii",
    "html",
];

/// Setup code to inject sandbox helpers into Python
pub const SANDBOX_SETUP_CODE: &str = r##"
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

# ============== Datetime Shim ==============
# Minimal datetime implementation for sandbox

class _DatetimeModule:
    """Namespace class acting as datetime module"""
    
    MINYEAR = 1
    MAXYEAR = 9999
    
    _DAYS_IN_MONTH = [0, 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    _DAY_NAMES = ['Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday', 'Sunday']
    _MONTH_NAMES = ['', 'January', 'February', 'March', 'April', 'May', 'June',
                    'July', 'August', 'September', 'October', 'November', 'December']
    
    @staticmethod
    def _is_leap(year):
        return year % 4 == 0 and (year % 100 != 0 or year % 400 == 0)
    
    @staticmethod
    def _days_in_month(year, month):
        if month == 2 and _DatetimeModule._is_leap(year):
            return 29
        return _DatetimeModule._DAYS_IN_MONTH[month]
    
    @staticmethod
    def _days_before_year(year):
        y = year - 1
        return y*365 + y//4 - y//100 + y//400
    
    @staticmethod
    def _days_before_month(year, month):
        days = sum(_DatetimeModule._DAYS_IN_MONTH[1:month])
        if month > 2 and _DatetimeModule._is_leap(year):
            days += 1
        return days
    
    @staticmethod
    def _ymd_to_ordinal(year, month, day):
        return _DatetimeModule._days_before_year(year) + _DatetimeModule._days_before_month(year, month) + day
    
    @staticmethod
    def _ordinal_to_ymd(n):
        n400, n = divmod(n, 146097)
        n100, n = divmod(n, 36524)
        n4, n = divmod(n, 1461)
        n1, n = divmod(n, 365)
        year = n400*400 + n100*100 + n4*4 + n1
        if n1 == 4 or n100 == 4:
            return year, 12, 31
        year += 1
        month = (n + 50) >> 5
        preceding = _DatetimeModule._days_before_month(year, month)
        if preceding > n:
            month -= 1
            preceding = _DatetimeModule._days_before_month(year, month)
        return year, month, n - preceding + 1

    class timedelta:
        def __init__(self, days=0, seconds=0, microseconds=0, milliseconds=0, minutes=0, hours=0, weeks=0):
            d = days + weeks * 7
            s = seconds + minutes * 60 + hours * 3600
            us = microseconds + milliseconds * 1000
            s, us = s + us // 1000000, us % 1000000
            d, s = d + s // 86400, s % 86400
            self._days, self._seconds, self._microseconds = int(d), int(s), int(us)
        
        @property
        def days(self): return self._days
        @property
        def seconds(self): return self._seconds
        @property
        def microseconds(self): return self._microseconds
        
        def total_seconds(self):
            return self._days * 86400 + self._seconds + self._microseconds / 1000000
        
        def __repr__(self):
            return f"datetime.timedelta(days={self._days}, seconds={self._seconds}, microseconds={self._microseconds})"
        
        def __eq__(self, other):
            if isinstance(other, _DatetimeModule.timedelta):
                return self._days == other._days and self._seconds == other._seconds and self._microseconds == other._microseconds
            return NotImplemented
        
        def __add__(self, other):
            if isinstance(other, _DatetimeModule.timedelta):
                return _DatetimeModule.timedelta(days=self._days + other._days, seconds=self._seconds + other._seconds, microseconds=self._microseconds + other._microseconds)
            return NotImplemented
        
        def __sub__(self, other):
            if isinstance(other, _DatetimeModule.timedelta):
                return _DatetimeModule.timedelta(days=self._days - other._days, seconds=self._seconds - other._seconds, microseconds=self._microseconds - other._microseconds)
            return NotImplemented
        
        def __neg__(self):
            return _DatetimeModule.timedelta(days=-self._days, seconds=-self._seconds, microseconds=-self._microseconds)

    class date:
        def __init__(self, year, month, day):
            if not 1 <= month <= 12:
                raise ValueError(f"month must be in 1..12")
            dim = _DatetimeModule._days_in_month(year, month)
            if not 1 <= day <= dim:
                raise ValueError(f"day is out of range for month")
            self._year, self._month, self._day = year, month, day
        
        @property
        def year(self): return self._year
        @property
        def month(self): return self._month
        @property
        def day(self): return self._day
        
        def weekday(self):
            return (_DatetimeModule._ymd_to_ordinal(self._year, self._month, self._day) + 6) % 7
        
        def isoweekday(self):
            return self.weekday() + 1
        
        def isoformat(self):
            return f"{self._year:04d}-{self._month:02d}-{self._day:02d}"
        
        def strftime(self, fmt):
            wd = self.weekday()
            r = fmt.replace('%Y', f"{self._year:04d}").replace('%m', f"{self._month:02d}").replace('%d', f"{self._day:02d}")
            r = r.replace('%B', _DatetimeModule._MONTH_NAMES[self._month]).replace('%b', _DatetimeModule._MONTH_NAMES[self._month][:3])
            r = r.replace('%A', _DatetimeModule._DAY_NAMES[wd]).replace('%a', _DatetimeModule._DAY_NAMES[wd][:3])
            return r.replace('%%', '%')
        
        def __repr__(self):
            return f"datetime.date({self._year}, {self._month}, {self._day})"
        
        def __str__(self):
            return self.isoformat()
        
        def __eq__(self, other):
            if isinstance(other, _DatetimeModule.date):
                return self._year == other._year and self._month == other._month and self._day == other._day
            return NotImplemented
        
        def __lt__(self, other):
            if isinstance(other, _DatetimeModule.date):
                return (self._year, self._month, self._day) < (other._year, other._month, other._day)
            return NotImplemented
        
        def __sub__(self, other):
            if isinstance(other, _DatetimeModule.date):
                d1 = _DatetimeModule._ymd_to_ordinal(self._year, self._month, self._day)
                d2 = _DatetimeModule._ymd_to_ordinal(other._year, other._month, other._day)
                return _DatetimeModule.timedelta(days=d1 - d2)
            elif isinstance(other, _DatetimeModule.timedelta):
                return self + _DatetimeModule.timedelta(days=-other.days)
            return NotImplemented
        
        def __add__(self, other):
            if isinstance(other, _DatetimeModule.timedelta):
                o = _DatetimeModule._ymd_to_ordinal(self._year, self._month, self._day) + other.days
                y, m, d = _DatetimeModule._ordinal_to_ymd(o)
                return _DatetimeModule.date(y, m, d)
            return NotImplemented

    class datetime(date):
        def __init__(self, year, month, day, hour=0, minute=0, second=0, microsecond=0):
            super().__init__(year, month, day)
            if not 0 <= hour <= 23: raise ValueError("hour out of range")
            if not 0 <= minute <= 59: raise ValueError("minute out of range")
            if not 0 <= second <= 59: raise ValueError("second out of range")
            if not 0 <= microsecond <= 999999: raise ValueError("microsecond out of range")
            self._hour, self._minute, self._second, self._microsecond = hour, minute, second, microsecond
        
        @property
        def hour(self): return self._hour
        @property
        def minute(self): return self._minute
        @property
        def second(self): return self._second
        @property
        def microsecond(self): return self._microsecond
        
        def date(self):
            return _DatetimeModule.date(self._year, self._month, self._day)
        
        def isoformat(self, sep='T'):
            d = f"{self._year:04d}-{self._month:02d}-{self._day:02d}"
            t = f"{self._hour:02d}:{self._minute:02d}:{self._second:02d}"
            if self._microsecond: t += f".{self._microsecond:06d}"
            return f"{d}{sep}{t}"
        
        def strftime(self, fmt):
            r = super().strftime(fmt)
            r = r.replace('%H', f"{self._hour:02d}").replace('%M', f"{self._minute:02d}").replace('%S', f"{self._second:02d}")
            r = r.replace('%I', f"{(self._hour % 12) or 12:02d}").replace('%p', 'PM' if self._hour >= 12 else 'AM')
            return r.replace('%f', f"{self._microsecond:06d}")
        
        def __repr__(self):
            return f"datetime.datetime({self._year}, {self._month}, {self._day}, {self._hour}, {self._minute}, {self._second}, {self._microsecond})"
        
        def __str__(self):
            return self.isoformat(' ')
        
        def __sub__(self, other):
            if isinstance(other, _DatetimeModule.datetime):
                d1, d2 = _DatetimeModule._ymd_to_ordinal(self._year, self._month, self._day), _DatetimeModule._ymd_to_ordinal(other._year, other._month, other._day)
                s1, s2 = self._hour*3600 + self._minute*60 + self._second, other._hour*3600 + other._minute*60 + other._second
                return _DatetimeModule.timedelta(days=d1-d2, seconds=s1-s2, microseconds=self._microsecond-other._microsecond)
            return super().__sub__(other)

# Register as a fake module via import hook
_datetime_instance = _DatetimeModule()
_datetime_instance.date = _DatetimeModule.date
_datetime_instance.datetime = _DatetimeModule.datetime
_datetime_instance.timedelta = _DatetimeModule.timedelta

# Restricted import with datetime shim
_sandbox_allowed_modules = {
    'math', 'json', 'random', 're', 'datetime', 'collections',
    'itertools', 'functools', 'operator', 'string', 'textwrap',
    'copy', 'types', 'typing', 'abc', 'numbers', 'decimal',
    'fractions', 'statistics', 'hashlib', 'base64', 'binascii',
    'html', '_sandbox', 'builtins'
}

_original_import = builtins.__import__

def _restricted_import(name, globals=None, locals=None, fromlist=(), level=0):
    # Handle datetime specially - return our shim
    if name == 'datetime':
        return _datetime_instance
    
    top_level = name.split('.')[0]
    if top_level not in _sandbox_allowed_modules:
        allowed_list = ', '.join(sorted(m for m in _sandbox_allowed_modules 
                                        if m not in ('_sandbox', 'builtins')))
        raise ImportError(
            f"Import '{name}' is not allowed in the sandbox. "
            f"Allowed modules: {allowed_list}. "
            f"For data analysis, use the built-in math, statistics, collections, and itertools modules."
        )
    return _original_import(name, globals, locals, fromlist, level)

builtins.__import__ = _restricted_import

# Clean up setup variables (but NOT _sandbox_allowed_modules - it's needed by the closure)
del _blocked, _name
"##;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{ToolModuleInfo, ToolFunctionInfo};
    
    #[test]
    fn test_generate_tool_module_code() {
        // Set up some tool modules
        let modules = vec![
            ToolModuleInfo {
                python_name: "bigquery".to_string(),
                server_id: "bigquery_server".to_string(),
                functions: vec![
                    ToolFunctionInfo {
                        name: "list_dataset_ids".to_string(),
                        description: Some("List datasets".to_string()),
                        parameters: serde_json::json!({}),
                    },
                ],
            },
        ];
        
        set_tool_modules(modules);
        let code = generate_tool_module_code();
        
        println!("Generated code:\n{}", code);
        
        // Verify the code contains expected elements
        assert!(code.contains("def list_dataset_ids("), "Should define global wrapper function");
        assert!(code.contains("tool_call(\"list_dataset_ids\""), "Should call tool_call with function name");
        assert!(code.contains("sys.modules['bigquery']"), "Should register module namespace");
        assert!(code.contains("class _bigquery:"), "Should create namespace class");
    }
    
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
