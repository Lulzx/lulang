use std::collections::HashMap;
use std::ffi::{c_char, c_void, CStr, CString};
use std::sync::{Mutex, OnceLock};

#[cfg(target_os = "linux")]
#[link(name = "dl")]
extern "C" {
    fn dlopen(path: *const c_char, mode: i32) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn dlerror() -> *const c_char;
}

#[cfg(target_os = "macos")]
extern "C" {
    fn dlopen(path: *const c_char, mode: i32) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn dlerror() -> *const c_char;
}

static HANDLES: OnceLock<Mutex<HashMap<String, usize>>> = OnceLock::new();

fn handles() -> &'static Mutex<HashMap<String, usize>> {
    HANDLES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn last_error() -> String {
    let error = unsafe { dlerror() };
    if error.is_null() {
        "dynamic loader returned no detail".into()
    } else {
        unsafe { CStr::from_ptr(error) }
            .to_string_lossy()
            .into_owned()
    }
}

fn library_candidates(lib: &str) -> Vec<String> {
    if lib.contains('/') || lib.ends_with(".so") || lib.ends_with(".dylib") {
        return vec![lib.into()];
    }
    #[cfg(target_os = "macos")]
    {
        vec![format!("lib{}.dylib", lib)]
    }
    #[cfg(target_os = "linux")]
    {
        vec![format!("lib{}.so", lib), format!("lib{}.so.6", lib)]
    }
}

fn open_library(lib: &str) -> Result<*mut c_void, String> {
    if let Some(&handle) = handles().lock().unwrap().get(lib) {
        return Ok(handle as *mut c_void);
    }
    let mut errors = Vec::new();
    for candidate in library_candidates(lib) {
        let path = CString::new(candidate.as_str())
            .map_err(|_| format!("library name contains NUL: {:?}", lib))?;
        let handle = unsafe { dlopen(path.as_ptr(), 1) };
        if !handle.is_null() {
            handles()
                .lock()
                .unwrap()
                .insert(lib.to_string(), handle as usize);
            return Ok(handle);
        }
        errors.push(format!("{}: {}", candidate, last_error()));
    }
    Err(format!(
        "cannot load FFI library `{}` ({})",
        lib,
        errors.join("; ")
    ))
}

pub fn resolve(lib: Option<&str>, symbol: &str) -> Result<usize, String> {
    let handle = match lib {
        Some(lib) => open_library(lib)?,
        #[cfg(target_os = "linux")]
        None => std::ptr::null_mut(),
        #[cfg(target_os = "macos")]
        None => (-2isize) as *mut c_void,
    };
    let symbol_name =
        CString::new(symbol).map_err(|_| format!("symbol contains NUL: {:?}", symbol))?;
    unsafe {
        dlerror();
    }
    let pointer = unsafe { dlsym(handle, symbol_name.as_ptr()) };
    if pointer.is_null() {
        Err(format!(
            "cannot resolve FFI symbol `{}`: {}",
            symbol,
            last_error()
        ))
    } else {
        Ok(pointer as usize)
    }
}

type I64Call = unsafe extern "C" fn(
    i64,
    i64,
    i64,
    i64,
    i64,
    i64,
    f64,
    f64,
    f64,
    f64,
    f64,
    f64,
    f64,
    f64,
) -> i64;
type F64Call = unsafe extern "C" fn(
    i64,
    i64,
    i64,
    i64,
    i64,
    i64,
    f64,
    f64,
    f64,
    f64,
    f64,
    f64,
    f64,
    f64,
) -> f64;

pub unsafe fn call_i64(pointer: usize, ints: [i64; 6], floats: [f64; 8]) -> i64 {
    let function: I64Call = std::mem::transmute(pointer);
    function(
        ints[0], ints[1], ints[2], ints[3], ints[4], ints[5], floats[0], floats[1], floats[2],
        floats[3], floats[4], floats[5], floats[6], floats[7],
    )
}

pub unsafe fn call_f64(pointer: usize, ints: [i64; 6], floats: [f64; 8]) -> f64 {
    let function: F64Call = std::mem::transmute(pointer);
    function(
        ints[0], ints[1], ints[2], ints[3], ints[4], ints[5], floats[0], floats[1], floats[2],
        floats[3], floats[4], floats[5], floats[6], floats[7],
    )
}
