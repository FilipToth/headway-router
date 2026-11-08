//! The graph a parsed map becomes.

use crate::{Cell, ViewBox};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;

macro_rules! id {
    ($name:ident, $what:literal) => {
        #[doc = concat!("Index of a ", $what, " within its `Network` vector.")]
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(pub usize);
    };
}
id!(StationId, "station");
id!(LineId, "line");
id!(EdgeId, "edge");

/// How Tennessine drew a station, which says how many lines call there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Marker {
    /// A white circle: two or more lines.
    Interchange,
    /// One tick per calling line, drawn in that line's colour.
    Tick { ticks: usize },
}

/// A facility glyph from the map's embedded `label-icons` font.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Airport,
    BusStation,
    Ferry,
    Spaceport,
    PlaceOfInterest,
    CableCar,
    TrainStation,
    TramStation,
}

impl Icon {
    /// The font maps these onto U+E800..=U+E807, in this order.
    pub fn from_char(c: char) -> Option<Self> {
        Some(match c {
            '\u{e800}' => Self::Airport,
            '\u{e801}' => Self::BusStation,
            '\u{e802}' => Self::Ferry,
            '\u{e803}' => Self::Spaceport,
            '\u{e804}' => Self::PlaceOfInterest,
            '\u{e805}' => Self::CableCar,
            '\u{e806}' => Self::TrainStation,
            '\u{e807}' => Self::TramStation,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone)]
pub struct Station {
    pub id: StationId,
    pub cell: Cell,
    /// `None` for the stations Tennessine left unlabelled. They are real stops
    /// and stay in the graph; they just cannot be searched for.
    pub name: Option<String>,
    pub icons: Vec<Icon>,
    pub marker: Marker,
    /// Every edge touching this station, including ones on unroutable lines.
    pub edges: Vec<EdgeId>,
}

impl Station {
    pub fn is_searchable(&self) -> bool {
        self.name.is_some()
    }
}

/// Inferred from the line's drawing style, as a starting point for speeds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// A plain stroke.
    Metro,
    /// A solid white stripe down the middle: Tennessine's tram styling.
    Tram,
    /// A dashed white stripe: commuter rail.
    CommuterRail,
}

/// The shape of a line's own subgraph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// Two termini and a single thread between them.
    Path,
    /// A closed loop.
    Cycle,
    /// Has a node of degree 3 or more. Tennessine cannot draw a branching
    /// line, so this means either two lines sharing one style or a line
    /// crossing itself. Not supported yet, so not routable.
    Branched,
    /// Drawn in several pieces that do not touch.
    Disconnected,
}

#[derive(Debug, Clone)]
pub struct Line {
    pub id: LineId,
    /// Joined with " / " when several legend entries share one style.
    pub name: String,
    /// Normally one name; more when the style is ambiguous.
    pub names: Vec<String>,
    /// Normalised for matching: whitespace stripped and lower-cased.
    pub colour: String,
    /// Every spelling of this line's core stroke as the file writes it, which
    /// is how a stylesheet addresses the line's paths. Normally just the one.
    pub strokes: Vec<String>,
    pub mode: Mode,
    pub shape: Shape,
    /// False when the line's shape is one we cannot route over yet.
    pub routable: bool,
    /// Stops in travel order, or empty when the shape is unusable. A cycle
    /// lists each stop once and does not repeat the first at the end.
    pub stops: Vec<StationId>,
    pub edges: Vec<EdgeId>,
}

impl Line {
    /// The sum of this line's drawn segment lengths, in grid units.
    pub fn length(&self, net: &Network) -> f64 {
        self.edges.iter().map(|e| net.edge(*e).length).sum()
    }
}

#[derive(Debug, Clone)]
pub struct Edge {
    pub id: EdgeId,
    pub line: LineId,
    pub a: StationId,
    pub b: StationId,
    /// The track as drawn, for rendering. Endpoints sit on the station cells
    /// but are offset from their centres by the corridor slot.
    pub polyline: Vec<(f64, f64)>,
    /// Drawn length in grid units.
    pub length: f64,
}

impl Edge {
    pub fn other(&self, from: StationId) -> StationId {
        if from == self.a {
            self.b
        } else {
            self.a
        }
    }
}

/// Everything the parser wants to tell you about a map it could not read
/// cleanly. None of these are fatal.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Warning {
    /// Without the key there are no line names, only colours.
    NoKeyToLines,
    /// Two or more legend entries are drawn identically, so their segments
    /// cannot be told apart. They are merged into one line.
    StyleSharedByLines { names: Vec<String>, colour: String },
    /// Track drawn in a style the key does not list.
    StyleNotInKey { colour: String, segments: usize },
    /// A line with a junction. Branching is not supported yet.
    LineBranches { line: String, at: Vec<String> },
    LineDisconnected { line: String, pieces: usize },
    /// A segment ending somewhere no station was drawn, so it cannot join the
    /// graph.
    DanglingSegment { colour: String, at: String },
    /// A segment whose two ends snap to the same station.
    SelfLoop { line: String, at: String },
    /// A station's tick colours disagree with the lines the graph says call
    /// there, which would mean the two layers were read inconsistently.
    TickColoursDisagree { station: String, ticks: Vec<String>, edges: Vec<String> },
    /// The same station was drawn twice. Harmless, and merged, but worth
    /// knowing about since Tennessine will not show it to you.
    StationDrawnTwice { station: String },
    /// Two *different* station markers landed on one grid cell, so one of them
    /// has been swallowed.
    StationsShareACell { at: String, names: Vec<String> },
    /// A station no routable line reaches.
    StationUnreachable { station: String },
    /// The routable network is not one connected piece.
    NetworkInPieces { pieces: usize },
}

impl fmt::Display for Warning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoKeyToLines => write!(
                f,
                "this map was exported without its key, so lines have no names"
            ),
            Self::StyleSharedByLines { names, colour } => write!(
                f,
                "{} are drawn identically ({colour}); their track cannot be told apart, so they \
                 are treated as one line",
                names.join(" and ")
            ),
            Self::StyleNotInKey { colour, segments } => write!(
                f,
                "{segments} segment(s) drawn in {colour} match no entry in the key"
            ),
            Self::LineBranches { line, at } => write!(
                f,
                "{line} branches at {} — branching lines are not supported yet, so {line} is \
                 excluded from routing",
                at.join(", ")
            ),
            Self::LineDisconnected { line, pieces } => write!(
                f,
                "{line} is drawn in {pieces} pieces that do not meet, so it is excluded from \
                 routing"
            ),
            Self::DanglingSegment { colour, at } => write!(
                f,
                "a {colour} segment ends at {at}, where no station is drawn"
            ),
            Self::SelfLoop { line, at } => {
                write!(f, "a {line} segment starts and ends at {at}")
            }
            Self::TickColoursDisagree { station, ticks, edges } => write!(
                f,
                "{station} has ticks for [{}] but track for [{}]",
                ticks.join(", "),
                edges.join(", ")
            ),
            Self::StationDrawnTwice { station } => {
                write!(f, "{station} is drawn twice in the same cell; the copies were merged")
            }
            Self::StationsShareACell { at, names } => write!(
                f,
                "two different stations ({}) share the cell at {at}, so only one survives",
                names.join(" and ")
            ),
            Self::StationUnreachable { station } => {
                write!(f, "{station} is not reachable: no routable line calls there")
            }
            Self::NetworkInPieces { pieces } => write!(
                f,
                "the routable network falls into {pieces} disconnected pieces, so some journeys \
                 are impossible"
            ),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Network {
    pub view_box: ViewBox,
    pub stations: Vec<Station>,
    pub lines: Vec<Line>,
    pub edges: Vec<Edge>,
    pub warnings: Vec<Warning>,
    /// Which station occupies each grid cell.
    pub by_cell: HashMap<Cell, StationId>,
}

impl Network {
    pub fn station(&self, id: StationId) -> &Station {
        &self.stations[id.0]
    }
    pub fn line(&self, id: LineId) -> &Line {
        &self.lines[id.0]
    }
    pub fn edge(&self, id: EdgeId) -> &Edge {
        &self.edges[id.0]
    }
    pub fn station_at(&self, cell: Cell) -> Option<StationId> {
        self.by_cell.get(&cell).copied()
    }

    /// A station's name, or a readable stand-in for the unlabelled ones.
    pub fn label(&self, id: StationId) -> String {
        let s = self.station(id);
        s.name.clone().unwrap_or_else(|| {
            let (x, y) = s.cell.centre();
            format!("(unnamed at {x}, {y})")
        })
    }

    pub fn is_routable(&self, edge: EdgeId) -> bool {
        self.line(self.edge(edge).line).routable
    }

    /// Stations reachable from `from` in one hop, over routable lines only.
    pub fn neighbours(&self, from: StationId) -> impl Iterator<Item = (StationId, &Edge)> + '_ {
        self.station(from)
            .edges
            .iter()
            .filter(|e| self.is_routable(**e))
            .map(move |e| {
                let edge = self.edge(*e);
                (edge.other(from), edge)
            })
    }

    pub fn routable_lines(&self) -> impl Iterator<Item = &Line> {
        self.lines.iter().filter(|l| l.routable)
    }

    /// Stations that no routable line calls at.
    pub fn unreachable_stations(&self) -> Vec<StationId> {
        self.stations
            .iter()
            .filter(|s| !s.edges.iter().any(|e| self.is_routable(*e)))
            .map(|s| s.id)
            .collect()
    }

    /// Connected pieces of the routable graph, largest first. Stations that no
    /// routable line reaches are left out entirely.
    pub fn components(&self) -> Vec<Vec<StationId>> {
        let mut seen = BTreeSet::new();
        let mut out: Vec<Vec<StationId>> = Vec::new();
        for s in &self.stations {
            if seen.contains(&s.id) || !s.edges.iter().any(|e| self.is_routable(*e)) {
                continue;
            }
            let mut stack = vec![s.id];
            let mut piece = Vec::new();
            while let Some(at) = stack.pop() {
                if !seen.insert(at) {
                    continue;
                }
                piece.push(at);
                stack.extend(self.neighbours(at).map(|(n, _)| n).filter(|n| !seen.contains(n)));
            }
            piece.sort();
            out.push(piece);
        }
        out.sort_by_key(|p| (std::cmp::Reverse(p.len()), p.first().copied()));
        out
    }

    /// Named stations, in a map from name to the stations carrying it. Several
    /// stations may legitimately share a name.
    pub fn search_index(&self) -> BTreeMap<&str, Vec<StationId>> {
        let mut index: BTreeMap<&str, Vec<StationId>> = BTreeMap::new();
        for s in &self.stations {
            if let Some(name) = s.name.as_deref() {
                index.entry(name).or_default().push(s.id);
            }
        }
        index
    }
}
