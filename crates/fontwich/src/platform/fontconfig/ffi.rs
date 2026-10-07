//! Calling libfontconfig either way: loaded at runtime, or linked normally.
//!
//! `yeslogic-fontconfig-sys` exposes the same API two ways. Loaded at runtime
//! the functions arrive as fields of a lazily loaded [`Library`] struct;
//! linked they are ordinary `extern "C"` items. Nothing else in this crate
//! should have to care which, so both shapes are hidden behind [`library()`]
//! and [`fc_call!`] here.
//!
//! Which one is in use is `build.rs`'s decision, from `RUST_FONTCONFIG_DLOPEN`
//! — not a Cargo feature, because a feature would unify across the build and
//! force the choice on every other crate in the graph. See `build.rs`.
//!
//! This is deliberately not `dlib::ffi_dispatch!`. That macro switches on a
//! `dlopen` feature belonging to whichever crate *invokes* it, so a caller
//! that spells its feature differently — as this crate does — silently gets
//! direct calls to symbols nothing linked. It compiles, because `cargo check`
//! does not link, and fails when something finally does.

#[cfg(fontconfig_dlopen)]
mod imp {
    /// The table of function pointers loaded from libfontconfig.
    pub(crate) use fontconfig_sys::Fc as Library;

    /// The loaded library, or `None` if it could not be found.
    ///
    /// `LIB_RESULT` rather than `LIB`: the latter panics when the library is
    /// missing, and a missing font library must not take the process down.
    pub(crate) fn library() -> Option<&'static Library> {
        fontconfig_sys::statics::LIB_RESULT.as_ref().ok()
    }
}

#[cfg(not(fontconfig_dlopen))]
mod imp {
    /// Nothing to carry: the functions are linked, so this is a placeholder
    /// that keeps every signature in the backend identical across the two
    /// modes. Zero-sized, so passing it around costs nothing.
    pub(crate) struct Library;

    /// Always available. The dynamic loader resolved libfontconfig before
    /// `main` ran, so a process that reaches this has it.
    pub(crate) fn library() -> Option<&'static Library> {
        Some(&Library)
    }
}

pub(crate) use imp::{Library, library};

/// Calls a fontconfig function through whichever mechanism is in use.
///
/// `fc_call!(fc, FcPatternCreate())` is `(fc.FcPatternCreate)()` when loading
/// at runtime and `FcPatternCreate()` when linked. Unsafe, like the calls it
/// stands for, and so expected to appear inside an `unsafe` block.
macro_rules! fc_call {
    ($lib:expr, $func:ident($($arg:expr),* $(,)?)) => {{
        #[cfg(fontconfig_dlopen)]
        let result = ($lib.$func)($($arg),*);
        #[cfg(not(fontconfig_dlopen))]
        let result = {
            // The placeholder is unused in this mode, but the call sites pass
            // it either way and an unused-variable warning helps no one.
            let _ = $lib;
            fontconfig_sys::$func($($arg),*)
        };
        result
    }};
}

pub(super) use fc_call;

/// Runs `f` holding the one lock every call into fontconfig takes.
///
/// fontconfig has been built to be thread safe since 2.10.91 — it carries
/// `fcatomic.h` and `fcmutex.h` for exactly that — but it has never documented
/// the guarantee, it has had races fixed since, and a crash in it is a crash
/// in the caller's process. Only a cache miss reaches fontconfig at all, a few
/// dozen times in the life of a process, so serializing them costs almost
/// nothing and turns "probably safe" into "safe". Remove it if the guarantee
/// is ever written down.
///
/// Lock order: the backend's cache lock may be held while taking this, never
/// the reverse.
pub(super) fn serialized<R>(f: impl FnOnce() -> R) -> R {
    static FONTCONFIG: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _held = FONTCONFIG
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    f()
}
