//! Turns a Tennessine Metro Designer SVG into a routable transit graph.
//!
//! Deliberately free of any web or wasm dependency: this crate is driven from
//! `cargo test` and from the browser alike. See `docs/tennessine-svg-format.md`
//! for the reverse-engineered format this is built on.

pub mod grid;
pub mod highlight;
pub mod network;
pub mod parse;
pub mod path;
pub mod route;
pub mod search;
pub mod svg;

pub use grid::Cell;
pub use highlight::{fade, trace};
pub use network::{Edge, EdgeId, Icon, Line, LineId, Marker, Mode, Network, Shape, Station, StationId, Warning};
pub use parse::{parse, ParseError};
pub use path::{parse_path, polyline_length, PathError};
pub use route::{fastest, Journey, Leg, RouteError};
pub use search::fold;
pub use svg::{parse_view_box, root_inner, root_inner_range, view_box, ViewBox};
