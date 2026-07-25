//! Cross-platform path-containment primitives.
//!
//! A VERBATIM Rust port of Orca's `src/shared/cross-platform-path.ts`
//! (@ v1.4.150-rc.0) — pure, lexical, hand-rolled posix + win32 path logic
//! with ZERO dependencies (no `regex` crate, no `std::path`; see Cargo.toml).
//!
//! # Security
//! [`is_path_inside_or_equal`] and [`relative_path_inside_root`] are
//! path-escape defenses used at containment boundaries. This module is PURELY
//! LEXICAL: it does NOT canonicalize the filesystem, resolve symlinks, or
//! resolve `..` segments during containment. See [`is_path_inside_or_equal`]
//! for the caller contract around `..`.

mod cross_platform_path;

pub use cross_platform_path::{
    get_runtime_path_basename, is_path_inside_or_equal, is_runtime_path_absolute,
    is_windows_absolute_path_like, normalize_runtime_path_for_comparison,
    normalize_runtime_path_separators, relative_path_inside_root, resolve_runtime_path, Flavor,
};
