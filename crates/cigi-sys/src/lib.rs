// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! The installer's only unsafe code: thin, safe functions over the Windows calls that std does
//! not expose. Every other crate in the workspace forbids unsafe code and calls these instead.
//! On every other platform this crate is empty.
//!
//! - `fs`: file identity, attributes, reparse points, ownership and access control, and the
//!   write-through rename the journal commits with.
//! - `env`: the user's `PATH` in `HKCU\Environment`, the broadcast that tells running programs it
//!   changed, and the Documents folder the PowerShell profiles live under.
//! - `policy`: the values of one `HKLM` key, read only, for the organization's machine settings.
//! - `process`: what the self-update and the shim need from the running process.
//!
//! Every `unsafe` block carries a `SAFETY:` comment saying why the call is sound.
#![cfg_attr(not(windows), forbid(unsafe_code))]
#![cfg_attr(windows, deny(unsafe_op_in_unsafe_fn))]

#[cfg(windows)]
pub mod env;
#[cfg(windows)]
pub mod fs;
#[cfg(windows)]
pub mod policy;
#[cfg(windows)]
pub mod process;
