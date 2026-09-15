//! Values read out of the current user's registry hive, for the Windows settings that have no API
//! of their own.

use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};

/// Returns the `REG_DWORD` stored at `sub_key` / `name` under `HKEY_CURRENT_USER`, or `None` when the
/// key or value is missing or holds any other type.
#[allow(
    unsafe_code,
    reason = "FFI to RegGetValueW. The data pointer targets a stack-local u32 whose size is the one handed in `pcbData`, RRF_RT_REG_DWORD refuses any value that isn't a 4-byte DWORD, and both name pointers are NUL-terminated buffers built here."
)]
pub(super) fn read_user_dword(sub_key: &str, name: &str) -> Option<u32> {
    let sub_key = nul_terminated_wide(sub_key);
    let name = nul_terminated_wide(name);
    let mut value: u32 = 0;
    // A DWORD is four bytes; the literal dodges the `cast_possible_truncation` a `size_of` would.
    let mut size: u32 = 4;

    // SAFETY: `sub_key` and `name` are NUL-terminated UTF-16 buffers that outlive the call. `value`
    // is a stack-local `u32` and `size` names its exact size, and `RRF_RT_REG_DWORD` makes the call
    // fail rather than write a value of any other type or length. The type out-parameter may be
    // null, and the API retains no pointer.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            sub_key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            std::ptr::from_mut::<u32>(&mut value).cast(),
            std::ptr::from_mut::<u32>(&mut size),
        )
    };
    (status == ERROR_SUCCESS).then_some(value)
}

fn nul_terminated_wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
#[path = "tests/registry_tests.rs"]
mod tests;
