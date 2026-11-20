//! Minimal Rust guest compiled to a Component Model component (Phase 4).
//!
//! JUDGMENT CALL: `#[export_name]` matches the canonical export names derived
//! from `wit/meld.wit`. `s64` lowers to core `i64`, so the hand-written guest
//! needs no allocator or WIT bindgen for this demo. A production Rust pack
//! would use `wit-bindgen`; the boundary contract is identical.
#![no_std]

#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    loop {}
}

#[export_name = "meld:demo/ops@0.1.0#add"]
pub extern "C" fn add(a: i64, b: i64) -> i64 {
    a.wrapping_add(b)
}

#[export_name = "meld:demo/ops@0.1.0#mul"]
pub extern "C" fn mul(a: i64, b: i64) -> i64 {
    a.wrapping_mul(b)
}
