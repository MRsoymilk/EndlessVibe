pub mod auth;
#[cfg(target_os = "linux")]
pub mod paths;
#[cfg(not(target_os = "linux"))]
#[path = "paths_portable.rs"]
pub mod paths;
// Compile and exercise the portable implementation on Linux as well.
#[cfg(all(target_os = "linux", test))]
#[path = "paths_portable.rs"]
mod paths_portable_test;
