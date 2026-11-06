//! Turns a Tennessine Metro Designer SVG into a routable transit graph.
//!
//! Deliberately free of any web or wasm dependency: this crate is driven from
//! `cargo test` and from the browser alike. See `docs/tennessine-svg-format.md`
//! for the reverse-engineered format this is built on.

pub mod grid;

pub use grid::Cell;
