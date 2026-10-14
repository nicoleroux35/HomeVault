/* =============================================================================
 * File          : mod.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Shared helpers for handlers: ProcParser reads live system information
 *   (CPU, memory, disks, network, sensors) from the kernel's /proc and /sys
 *   virtual filesystems, and ResponseHelper/JsonResponse build the uniform
 *   JSON answer every endpoint returns.
 *
 * Dependencies:
 *   - none (re-exports only)
 *
 * Usage:
 *   `use crate::utils::{ProcParser, ResponseHelper};` inside handlers —
 *   see the submodule documentation for details.
 * =============================================================================
 */

//! Shared handler helpers: Linux system-information parsing (`proc`) and the
//! JSON response envelope (`response`).

pub mod proc;
pub mod response;

pub use proc::ProcParser;
pub use response::{JsonResponse, ResponseHelper};
