// Prevents additional console window on Windows in release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // First line: anchors the startup trace as close to process entry as
    // possible (dyld pre-main time is still outside this anchor; measure that
    // externally when needed).
    sift_lib::startup_trace::begin();
    sift_lib::startup_trace::mark("main");
    sift_lib::run()
}
