// Native helpers the JIT-compiled code calls. Arrays are a length header (i64)
// followed by 8-byte elements; allocations are leaked (benchmark-lifetime model,
// real memory management arrives with the value-semantics IR).
use std::alloc::{alloc, Layout};

pub extern "C" fn lu_print_f64(v: f64) {
    print!("{}", v);
}
pub extern "C" fn lu_print_i64(v: i64) {
    print!("{}", v);
}
pub extern "C" fn lu_print_bool(v: i64) {
    print!("{}", v != 0);
}
pub extern "C" fn lu_print_str(ptr: *const u8, len: i64) {
    let s = unsafe { std::slice::from_raw_parts(ptr, len as usize) };
    print!("{}", String::from_utf8_lossy(s));
}
pub extern "C" fn lu_print_sep() {
    print!(" ");
}
pub extern "C" fn lu_print_nl() {
    println!();
}

fn arr_alloc(n: i64) -> *mut u8 {
    let bytes = 8 + (n as usize) * 8;
    let layout = Layout::from_size_align(bytes, 8).expect("bad array layout");
    let p = unsafe { alloc(layout) };
    if p.is_null() {
        eprintln!("error: out of memory allocating array of {} elements", n);
        std::process::exit(1);
    }
    unsafe { *(p as *mut i64) = n };
    p
}

pub extern "C" fn lu_arr_new_f64(n: i64, init: f64) -> *mut u8 {
    let p = arr_alloc(n);
    let data = unsafe { (p.add(8)) as *mut f64 };
    for i in 0..n as usize {
        unsafe { *data.add(i) = init };
    }
    p
}

pub extern "C" fn lu_arr_new_i64(n: i64, init: i64) -> *mut u8 {
    let p = arr_alloc(n);
    let data = unsafe { (p.add(8)) as *mut i64 };
    for i in 0..n as usize {
        unsafe { *data.add(i) = init };
    }
    p
}

/// Uninitialized array of `n` 8-byte slots (JIT emits the fill loop — record
/// arrays are laid out SoA, a decision the compiler owns, not the runtime).
pub extern "C" fn lu_arr_new_raw(n: i64) -> *mut u8 {
    arr_alloc(n)
}

pub extern "C" fn lu_str_eq(ap: *const u8, al: i64, bp: *const u8, bl: i64) -> i64 {
    if al != bl {
        return 0;
    }
    let a = unsafe { std::slice::from_raw_parts(ap, al as usize) };
    let b = unsafe { std::slice::from_raw_parts(bp, bl as usize) };
    (a == b) as i64
}

pub extern "C" fn lu_oob(idx: i64, len: i64) {
    eprintln!("error: index {} out of bounds (length {})", idx, len);
    std::process::exit(1);
}

pub extern "C" fn lu_sin(x: f64) -> f64 {
    x.sin()
}
pub extern "C" fn lu_cos(x: f64) -> f64 {
    x.cos()
}
pub extern "C" fn lu_acos(x: f64) -> f64 {
    x.acos()
}
pub extern "C" fn lu_atan2(a: f64, b: f64) -> f64 {
    a.atan2(b)
}
pub extern "C" fn lu_pow(a: f64, b: f64) -> f64 {
    a.powf(b)
}
