//! Sift's native GPUI application, as a library plus a thin binary.
//!
//! Exposed as a library for two reasons: integration tests need to construct the
//! real view (a `tests/` target cannot import from a binary crate), and the
//! window bootstrap in `main.rs` stays a three-line file that only decides where
//! the store lives and how big the window opens.
//!
//! The model, the treemap layout and the service wiring deliberately expose a
//! little more than the current view uses: the drawing surface is still growing
//! into them, and every one of those items is covered by unit tests. A crate's
//! unused-API lint cannot see "tested and exercised, just not wired to a button
//! yet", so the allow is scoped here, with this reason, rather than sprinkled
//! over the items.
#![allow(dead_code)]

pub mod app;
pub mod assets;
pub mod model;
pub mod services;
pub mod theme;
pub mod treemap_element;
