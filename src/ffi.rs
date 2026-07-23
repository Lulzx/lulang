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
static PREPARED: OnceLock<Mutex<usize>> = OnceLock::new();

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
    let bridge = match symbol {
        "lu_ffi_prepare" => Some(lu_ffi_prepare as *const () as usize),
        "lu_ffi_call_i" => Some(lu_ffi_call_i as *const () as usize),
        "lu_ffi_call_f" => Some(lu_ffi_call_f as *const () as usize),
        _ => None,
    };
    if let Some(pointer) = bridge {
        return Ok(pointer);
    }
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

fn bytes_arg(pointer: *const u8, length: i64) -> Result<String, String> {
    if length < 0 || (pointer.is_null() && length != 0) {
        return Err("invalid FFI bridge string".into());
    }
    let bytes = unsafe { std::slice::from_raw_parts(pointer, length as usize) };
    std::str::from_utf8(bytes)
        .map(String::from)
        .map_err(|_| "FFI bridge string is not UTF-8".into())
}

pub extern "C" fn lu_ffi_prepare(
    library: *const u8,
    library_length: i64,
    symbol: *const u8,
    symbol_length: i64,
) -> i64 {
    let result = (|| {
        let library = bytes_arg(library, library_length)?;
        let symbol = bytes_arg(symbol, symbol_length)?;
        resolve((!library.is_empty()).then_some(library.as_str()), &symbol)
    })();
    match result {
        Ok(pointer) => {
            *PREPARED.get_or_init(|| Mutex::new(0)).lock().unwrap() = pointer;
            1
        }
        Err(error) => {
            eprintln!("runtime error: {}", error);
            0
        }
    }
}

unsafe fn unpack_call(
    control_pointer: *mut i64,
    control_length: i64,
    float_pointer: *mut f64,
    float_length: i64,
) -> Result<([i64; 6], [f64; 8], Vec<Vec<u8>>), String> {
    if control_length < 1 || float_length < 0 {
        return Err("invalid packed FFI call".into());
    }
    let control = std::slice::from_raw_parts_mut(control_pointer, control_length as usize);
    let floats = std::slice::from_raw_parts_mut(float_pointer, float_length as usize);
    let arguments = usize::try_from(control[0]).map_err(|_| "invalid packed argument count")?;
    if 1 + arguments * 3 > control.len() {
        return Err("truncated packed FFI descriptors".into());
    }
    let mut ints = [0i64; 6];
    let mut float_registers = [0.0f64; 8];
    let mut integer_index = 0usize;
    let mut float_index = 0usize;
    let mut string_buffers = Vec::new();
    for argument in 0..arguments {
        let descriptor = 1 + argument * 3;
        let kind = control[descriptor];
        let value = control[descriptor + 1];
        let length = control[descriptor + 2];
        match kind {
            0 => {
                if integer_index >= ints.len() {
                    return Err("packed FFI integer register overflow".into());
                }
                ints[integer_index] = value;
                integer_index += 1;
            }
            1 => {
                let offset = usize::try_from(value).map_err(|_| "invalid float offset")?;
                if float_index >= float_registers.len() || offset >= floats.len() {
                    return Err("packed FFI float register overflow".into());
                }
                float_registers[float_index] = floats[offset];
                float_index += 1;
            }
            2 => {
                let offset = usize::try_from(value).map_err(|_| "invalid string data offset")?;
                let length = usize::try_from(length).map_err(|_| "invalid string data length")?;
                if integer_index + 2 > ints.len() || offset + length > control.len() {
                    return Err("packed FFI string data is out of bounds".into());
                }
                let bytes = control[offset..offset + length]
                    .iter()
                    .map(|value| *value as u8)
                    .collect::<Vec<_>>();
                ints[integer_index] = bytes.as_ptr() as i64;
                ints[integer_index + 1] = length as i64;
                integer_index += 2;
                string_buffers.push(bytes);
            }
            3 => {
                let offset = usize::try_from(value).map_err(|_| "invalid integer data offset")?;
                let length = usize::try_from(length).map_err(|_| "invalid integer data length")?;
                if integer_index + 2 > ints.len() || offset + length > control.len() {
                    return Err("packed FFI integer data is out of bounds".into());
                }
                ints[integer_index] = control.as_mut_ptr().add(offset) as i64;
                ints[integer_index + 1] = length as i64;
                integer_index += 2;
            }
            4 => {
                let offset = usize::try_from(value).map_err(|_| "invalid float data offset")?;
                let length = usize::try_from(length).map_err(|_| "invalid float data length")?;
                if integer_index + 2 > ints.len() || offset + length > floats.len() {
                    return Err("packed FFI float data is out of bounds".into());
                }
                ints[integer_index] = floats.as_mut_ptr().add(offset) as i64;
                ints[integer_index + 1] = length as i64;
                integer_index += 2;
            }
            _ => return Err(format!("unknown packed FFI argument kind {}", kind)),
        }
    }
    Ok((ints, float_registers, string_buffers))
}

pub unsafe extern "C" fn lu_ffi_call_i(
    control: *mut i64,
    control_length: i64,
    floats: *mut f64,
    float_length: i64,
) -> i64 {
    let pointer = *PREPARED.get_or_init(|| Mutex::new(0)).lock().unwrap();
    match unpack_call(control, control_length, floats, float_length) {
        Ok((ints, float_registers, _strings)) if pointer != 0 => {
            call_i64(pointer, ints, float_registers)
        }
        Ok(_) => {
            eprintln!("runtime error: no prepared FFI symbol");
            0
        }
        Err(error) => {
            eprintln!("runtime error: {}", error);
            0
        }
    }
}

pub unsafe extern "C" fn lu_ffi_call_f(
    control: *mut i64,
    control_length: i64,
    floats: *mut f64,
    float_length: i64,
) -> f64 {
    let pointer = *PREPARED.get_or_init(|| Mutex::new(0)).lock().unwrap();
    match unpack_call(control, control_length, floats, float_length) {
        Ok((ints, float_registers, _strings)) if pointer != 0 => {
            call_f64(pointer, ints, float_registers)
        }
        Ok(_) => {
            eprintln!("runtime error: no prepared FFI symbol");
            0.0
        }
        Err(error) => {
            eprintln!("runtime error: {}", error);
            0.0
        }
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
