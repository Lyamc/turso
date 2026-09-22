// NB libc does not define this constant on all platforms, so we hard code it
// like CPython does.
// https://github.com/python/cpython/blob/d8b9011702443bb57579f8834f3effe58e290dfc/Include/pyport.h#L372
pub const INT_MAX: core::ffi::c_int = 2147483647;

pub type PY_UINT32_T = u32;
pub type PY_UINT64_T = u64;

pub type PY_INT32_T = i32;
pub type PY_INT64_T = i64;

pub type size_t = usize;
pub type ssize_t = isize;
pub type uintptr_t = usize;
pub type intptr_t = isize;

// Match the platform C `wchar_t`: 16-bit on Windows, 32-bit elsewhere.
#[cfg(windows)]
pub type wchar_t = u16;
#[cfg(not(windows))]
pub type wchar_t = i32;

/// Opaque stand-in for C `FILE`. PyO3 only ever passes pointers to it.
#[repr(C)]
pub struct FILE {
    _opaque: [u8; 0],
    _marker: core::marker::PhantomData<(*mut u8, core::marker::PhantomPinned)>,
}

pub type Py_uintptr_t = uintptr_t;
pub type Py_intptr_t = intptr_t;
pub type Py_ssize_t = ssize_t;

pub type Py_hash_t = Py_ssize_t;
pub type Py_uhash_t = size_t;

pub const PY_SSIZE_T_MIN: Py_ssize_t = Py_ssize_t::MIN;
pub const PY_SSIZE_T_MAX: Py_ssize_t = Py_ssize_t::MAX;

#[cfg(target_endian = "big")]
pub const PY_BIG_ENDIAN: usize = 1;
#[cfg(target_endian = "big")]
pub const PY_LITTLE_ENDIAN: usize = 0;

#[cfg(target_endian = "little")]
pub const PY_BIG_ENDIAN: usize = 0;
#[cfg(target_endian = "little")]
pub const PY_LITTLE_ENDIAN: usize = 1;

use core::ffi::c_char;

unsafe extern "C" {
    pub fn getenv(name: *const c_char) -> *mut c_char;
}
