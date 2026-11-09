//! Turns a Tennessine Metro Designer SVG into a routable transit graph.
//!
//! Deliberately free of any web or wasm dependency: this crate is driven from
//! `cargo test` and from the browser alike. See `docs/tennessine-svg-format.md`
//! for the reverse-engineered format this is built on.

pub mod config;
pub mod grid;
pub mod highlight;
pub mod network;
pub mod parse;
pub mod path;
pub mod reach;
pub mod route;
pub mod scale;
pub mod search;
pub mod service;
pub mod svg;
pub mod timing;

pub use config::{Config, ConfigError};
pub use grid::Cell;
pub use highlight::{fade, tag, trace};
pub use network::{Edge, EdgeId, Icon, Line, LineId, Marker, Mode, Network, Shape, Station, StationId, Warning};
pub use parse::{parse, ParseError};
pub use path::{parse_path, polyline_length, PathError};
pub use reach::{Fringe, Spread, Stop};
pub use route::{fastest, options, Choice, Journey, Leg, Merit, RouteError};
pub use scale::Scale;
pub use search::fold;
pub use service::{Category, Services};
pub use svg::{parse_view_box, root_inner, root_inner_range, view_box, ViewBox};
pub use timing::Timing;
