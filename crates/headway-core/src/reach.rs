//! How far you can get, rather than how to get to one place.
//!
//! A router answers "how do I get from here to there". This answers the other
//! question people ask of a transit map — "what is within half an hour of
//! here?" — and it is the same search run to exhaustion instead of stopped at
//! a target. Deliberately the same search: an isochrone that disagreed with
//! the itinerary about how long a journey takes would make a liar of one of
//! them, so both read their seconds from [`Timing`] and neither has a cost
//! model of its own.
//!
//! [`Spread::from`] does the work once, for the whole map and with no limit.
//! The limit is applied afterwards by [`Spread::stops`] and
//! [`Spread::fringe`], which is what makes dragging the minutes figure around
//! feel free: changing it re-slices a table rather than re-searching the city.
//!
//! Drawing an isochrone on a schematic map is its own problem. There are no
//! areas here, only track, so the frontier is drawn *along the lines*: every
//! stretch inside the limit in its band's colour, and the stretch that runs
//! out mid-segment cut where the time goes. That is why [`Spread::fringe`]
//! hands back geometry rather than a list of stations — the list alone says
//! what is reachable, but the cut track is what shows you the shape of it.

use crate::highlight::LEG_CLASS;
use crate::network::{EdgeId, LineId, Network, StationId};
use crate::path::polyline_length;
use crate::route::Order;
use crate::timing::Timing;
use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap};

/// How many bands an isochrone is drawn in. Four reads as a ramp without
/// asking anyone to tell eight colours apart.
pub const BANDS: usize = 4;

/// The ink for each band, nearest first.
///
/// A sequence rather than a palette: blue through teal and green to amber,
/// mid-dark throughout so that every band reads against Tennessine's pale
/// paper, and ordered so that "further out" is obvious even before the legend
/// is read.
pub const BAND_INK: [&str; BANDS] = ["#1b4ea8", "#0f8f96", "#5a9a2b", "#c8761b"];

/// Width of banded track, against the map's own 0.2 track.
pub const SPREAD_WIDTH: f64 = 0.3;
/// A dark edge under it, for the same reason [`crate::highlight::CASING`]
/// exists: the band colours have to read the same over every line they cover.
pub const SPREAD_CASING: f64 = 0.42;
/// A dot at every station inside the limit, and a ring at the one it is all
/// measured from.
pub const STOP_RADIUS: f64 = 0.15;
pub const ORIGIN_RADIUS: f64 = 0.3;
pub const RING_WIDTH: f64 = 0.1;

/// A stamped minute figure: its text height in grid units, how far it stands
/// above the dot it belongs to, and the white it is outlined in so that it
/// reads over whatever track it lands on.
pub const STAMP_SIZE: f64 = 0.44;
pub const STAMP_OFFSET: f64 = 0.3;
pub const STAMP_HALO: f64 = 0.14;

// The same guards the route layer keeps: a band drawn at the map's own track
// width is not an overlay, and a casing has to be wider than what it carries.
const _: () = assert!(SPREAD_WIDTH > 0.2);
const _: () = assert!(SPREAD_CASING > SPREAD_WIDTH);

/// Which band a time falls in under a limit, 0 being the nearest.
///
/// The bands split the limit evenly, and the limit itself belongs to the last
/// one rather than falling off the end of the ramp.
pub fn band(seconds: f64, limit: f64) -> usize {
    if !limit.is_finite() || limit <= 0.0 || !seconds.is_finite() {
        return 0;
    }
    let step = (seconds.max(0.0) / limit * BANDS as f64).floor();
    (step as usize).min(BANDS - 1)
}

/// A station inside an isochrone.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stop {
    pub station: StationId,
    /// Where its marker sits on the map.
    pub at: (f64, f64),
    pub seconds: f64,
    pub band: usize,
}

/// A stretch of track inside an isochrone, as it is to be drawn.
#[derive(Debug, Clone, PartialEq)]
pub struct Fringe {
    /// Running away from the origin, cut where the time runs out.
    pub points: Vec<(f64, f64)>,
    /// Seconds from the origin at the end it starts from, which is the band it
    /// is drawn in.
    pub seconds: f64,
    pub band: usize,
}

/// How long every station on a map is from one station.
#[derive(Debug, Clone, PartialEq)]
pub struct Spread {
    origin: StationId,
    /// Seconds to reach each station, by station index. `None` where nothing
    /// the planner may ride gets there at all.
    seconds: Vec<Option<f64>>,
    /// The best arrival at each station *on each line*.
    ///
    /// Kept because the arrival time alone cannot say what riding on from a
    /// station costs: staying aboard costs a stand, changing costs the walk
    /// and the wait, and which of the two it is depends on what you came in
    /// on. [`Spread::departure`] is the only reason this is here, and the
    /// frontier is wrong without it.
    aboard: BTreeMap<(StationId, LineId), f64>,
}

impl Spread {
    /// Times every station on the map from `origin`.
    ///
    /// One exhaustive Dijkstra over (station, line aboard), exactly as
    /// [`crate::fastest`] runs it and over the same lines — so a station's time
    /// here is the time the itinerary would quote for going there, to the
    /// second.
    pub fn from(timing: &Timing, origin: StationId) -> Self {
        let net = timing.net();
        let mut seconds: Vec<Option<f64>> = vec![None; net.stations.len()];
        if let Some(slot) = seconds.get_mut(origin.0) {
            *slot = Some(0.0);
        }

        // Where you are and what you are aboard, as the planner searches it.
        type State = (StationId, LineId);
        let mut best: BTreeMap<State, f64> = BTreeMap::new();
        let mut queue: BinaryHeap<Reverse<(Order, State)>> = BinaryHeap::new();

        for (next, edge) in net.neighbours(origin) {
            if !timing.rides(edge.line) {
                continue;
            }
            let state = (next, edge.line);
            let cost = timing.step(edge.id, None);
            if best.get(&state).is_none_or(|had| cost < *had) {
                best.insert(state, cost);
                queue.push(Reverse((Order(cost), state)));
            }
        }

        while let Some(Reverse((Order(cost), state))) = queue.pop() {
            if cost > best.get(&state).copied().unwrap_or(f64::INFINITY) {
                continue; // a quicker way here was already settled
            }
            let (at, aboard) = state;
            // The arrival a passenger cares about is the best over every line
            // they could have come in on.
            if let Some(slot) = seconds.get_mut(at.0) {
                if slot.is_none_or(|had| cost < had) {
                    *slot = Some(cost);
                }
            }
            for (next, edge) in net.neighbours(at) {
                if !timing.rides(edge.line) {
                    continue;
                }
                let onward = (next, edge.line);
                let total = cost + timing.step(edge.id, Some(aboard));
                if best.get(&onward).is_none_or(|had| total < *had) {
                    best.insert(onward, total);
                    queue.push(Reverse((Order(total), onward)));
                }
            }
        }

        Self { origin, seconds, aboard: best }
    }

    pub fn origin(&self) -> StationId {
        self.origin
    }

    /// Where the isochrone is measured from, on the map.
    pub fn start(&self, net: &Network) -> (f64, f64) {
        net.station(self.origin).cell.centre()
    }

    /// Seconds to reach a station, or `None` if nothing rideable gets there.
    pub fn seconds(&self, station: StationId) -> Option<f64> {
        self.seconds.get(station.0).copied().flatten()
    }

    /// How many stations can be reached at all, counting the origin.
    pub fn reached(&self) -> usize {
        self.seconds.iter().filter(|s| s.is_some()).count()
    }

    /// The stations inside `limit` seconds, soonest first.
    pub fn stops(&self, net: &Network, limit: f64) -> Vec<Stop> {
        let mut out: Vec<Stop> = self
            .seconds
            .iter()
            .enumerate()
            .filter_map(|(i, held)| {
                let seconds = held.filter(|t| *t <= limit)?;
                let station = StationId(i);
                Some(Stop {
                    station,
                    at: net.station(station).cell.centre(),
                    seconds,
                    band: band(seconds, limit),
                })
            })
            .collect();
        out.sort_by(|a, b| {
            a.seconds
                .partial_cmp(&b.seconds)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.station.cmp(&b.station))
        });
        out
    }

    /// How many stations fall in each band, nearest band first.
    pub fn counts(&self, limit: f64) -> [usize; BANDS] {
        let mut out = [0usize; BANDS];
        for seconds in self.seconds.iter().flatten().filter(|t| **t <= limit) {
            out[band(*seconds, limit)] += 1;
        }
        out
    }

    /// The earliest you can be *rolling* out of a station along one segment.
    ///
    /// Not the same as arriving there, and that difference is the whole of it:
    /// a junction reached at twelve minutes is twelve minutes from the lines
    /// you are already aboard and twelve minutes *plus a change* from every
    /// other line through it. So this adds what boarding this segment's line
    /// costs — a stand if you are on it already, the walk and the wait if you
    /// are not — taking whichever way in makes that cheapest.
    ///
    /// `None` when nothing rideable gets to the station at all. The origin
    /// departs at nought: a journey's first boarding is not charged a wait,
    /// exactly as the planner seeds its own search.
    pub fn departure(&self, timing: &Timing, from: StationId, edge: EdgeId) -> Option<f64> {
        if from == self.origin {
            return Some(0.0);
        }
        let net = timing.net();
        let onto = net.edge(edge).line;
        net.station(from)
            .edges
            .iter()
            .filter_map(|held| {
                let line = net.edge(*held).line;
                let arrived = *self.aboard.get(&(from, line))?;
                Some(match line == onto {
                    true => arrived + timing.dwell,
                    false => arrived + timing.boarding(onto),
                })
            })
            .fold(None, |best: Option<f64>, when| {
                Some(match best {
                    Some(had) => had.min(when),
                    None => when,
                })
            })
    }

    /// Every stretch of track inside the limit, each cut where the time goes.
    ///
    /// A segment is approached from both ends, because a point in the middle of
    /// one is reached by riding in from whichever station is nearer in time. So
    /// a long hop can be drawn as two stubs with a gap between them — which is
    /// the honest picture: that gap is track you cannot be standing on inside
    /// the limit.
    ///
    /// What "nearer in time" means is [`Spread::departure`] and not the
    /// arrival: measuring from the arrival hands every line through a junction
    /// the change that reaching it cost nothing for, and draws the frontier out
    /// past stations the limit does not include.
    ///
    /// Where the cut lands is interpolated along the drawn track, which treats
    /// a segment as covered at a steady rate. It is not: a train spends its
    /// first seconds accelerating, so the true frontier sits a little short of
    /// the mark. The stations either side of it are exact, and they are what
    /// anyone reads a time off.
    pub fn fringe(&self, timing: &Timing, limit: f64) -> Vec<Fringe> {
        let net = timing.net();
        let mut out = Vec::new();
        for edge in &net.edges {
            if !timing.rides(edge.line) || edge.polyline.len() < 2 {
                continue;
            }
            let ride = timing.edge(edge.id);
            // Measured from when a train can actually be rolling out of each
            // end, not from when a passenger lands there.
            let (from_a, from_b) = (
                self.departure(timing, edge.a, edge.id),
                self.departure(timing, edge.b, edge.id),
            );
            let share = |held: Option<f64>| -> f64 {
                match held {
                    Some(t) if t <= limit && ride > 0.0 => ((limit - t) / ride).clamp(0.0, 1.0),
                    // A segment with no time to it is in or out whole.
                    Some(t) if t <= limit => 1.0,
                    _ => 0.0,
                }
            };
            let (reach_a, reach_b) = (share(from_a), share(from_b));
            if reach_a <= 0.0 && reach_b <= 0.0 {
                continue;
            }

            // The two stubs meet or overlap: the whole segment is inside, and
            // it is banded by the end that is nearer the origin.
            if reach_a + reach_b >= 1.0 {
                let near = from_a
                    .into_iter()
                    .chain(from_b)
                    .fold(f64::INFINITY, f64::min);
                out.push(Fringe {
                    points: edge.polyline.clone(),
                    seconds: near,
                    band: band(near, limit),
                });
                continue;
            }
            if let (Some(t), true) = (from_a, reach_a > 0.0) {
                out.push(Fringe {
                    points: cut(&edge.polyline, reach_a),
                    seconds: t,
                    band: band(t, limit),
                });
            }
            if let (Some(t), true) = (from_b, reach_b > 0.0) {
                let mut backwards = edge.polyline.clone();
                backwards.reverse();
                out.push(Fringe {
                    points: cut(&backwards, reach_b),
                    seconds: t,
                    band: band(t, limit),
                });
            }
        }
        out
    }
}

/// The first `share` of a polyline by length, interpolating the last point.
///
/// Returns at least two points for any share above zero, because a stub of one
/// point draws nothing and would read as a gap in the isochrone.
fn cut(points: &[(f64, f64)], share: f64) -> Vec<(f64, f64)> {
    let total = polyline_length(points);
    if points.len() < 2 || total <= 0.0 || share >= 1.0 {
        return points.to_vec();
    }
    let want = total * share.max(0.0);
    let mut out = vec![points[0]];
    let mut walked = 0.0;
    for pair in points.windows(2) {
        let (from, to) = (pair[0], pair[1]);
        let step = (to.0 - from.0).hypot(to.1 - from.1);
        if walked + step >= want {
            let along = match step > 0.0 {
                true => (want - walked) / step,
                false => 0.0,
            };
            out.push((
                from.0 + (to.0 - from.0) * along,
                from.1 + (to.1 - from.1) * along,
            ));
            return out;
        }
        walked += step;
        out.push(to);
    }
    out
}

/// The isochrone as a layer to drop over the map: banded track under a dot at
/// every station reached, and a ring where it all spreads from.
///
/// Marked with [`LEG_CLASS`] like the route layer, so that the fade rules step
/// around it rather than greying out the very thing being shown.
pub fn layer(fringes: &[Fringe], stops: &[Stop], start: (f64, f64)) -> String {
    let mut svg = String::new();
    // Casings first, all of them, so that one band's edge never cuts across
    // the band beside it.
    for piece in fringes {
        svg.push_str(&stroke(piece, "#23232b", SPREAD_CASING));
    }
    for piece in fringes {
        svg.push_str(&stroke(piece, BAND_INK[piece.band.min(BANDS - 1)], SPREAD_WIDTH));
    }
    for stop in stops {
        svg.push_str(&format!(
            "<circle class=\"{LEG_CLASS}\" cx=\"{:.4}\" cy=\"{:.4}\" r=\"{STOP_RADIUS}\" \
             fill=\"{}\" stroke=\"#fff\" stroke-width=\"{:.4}\"/>",
            stop.at.0,
            stop.at.1,
            BAND_INK[stop.band.min(BANDS - 1)],
            STOP_RADIUS * 0.55,
        ));
    }
    svg.push_str(&format!(
        "<circle class=\"{LEG_CLASS}\" cx=\"{:.4}\" cy=\"{:.4}\" r=\"{ORIGIN_RADIUS}\" \
         fill=\"#fff\" stroke=\"#23232b\" stroke-width=\"{RING_WIDTH}\"/>",
        start.0, start.1,
    ));
    svg
}

/// How long it takes to get to each station, written beside it.
///
/// The map's own answer to "and how long to there?", which the bands can only
/// give to the nearest quarter of the limit. Whole minutes, because a second
/// figure on a schematic map is a figure nobody can read: the panel has the
/// exact times for the stations near the frontier, where they matter.
pub fn stamp(stops: &[Stop]) -> String {
    let mut svg = String::new();
    for stop in stops {
        let minutes = (stop.seconds / 60.0).round() as u64;
        svg.push_str(&format!(
            "<text class=\"{LEG_CLASS}\" x=\"{:.4}\" y=\"{:.4}\" \
             font-size=\"{STAMP_SIZE}\" text-anchor=\"middle\" \
             fill=\"{}\" stroke=\"#fff\" stroke-width=\"{STAMP_HALO}\" \
             paint-order=\"stroke\">{minutes}</text>",
            stop.at.0,
            stop.at.1 - STAMP_OFFSET,
            BAND_INK[stop.band.min(BANDS - 1)],
        ));
    }
    svg
}

fn stroke(piece: &Fringe, colour: &str, width: f64) -> String {
    let mut d = String::new();
    for (i, (x, y)) in piece.points.iter().enumerate() {
        d.push(if i == 0 { 'M' } else { 'L' });
        d.push_str(&format!("{x:.4} {y:.4}"));
        if i + 1 < piece.points.len() {
            d.push(' ');
        }
    }
    format!(
        "<path class=\"{LEG_CLASS}\" d=\"{d}\" fill=\"none\" stroke=\"{colour}\" \
         stroke-width=\"{width}\" stroke-linecap=\"round\" stroke-linejoin=\"round\"/>"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{fastest, parse, Scale, Services, Timing};

    /// A straight line of four stations, and a branch line off the middle one.
    ///
    /// ```text
    ///   Pier --North-- Quay --North-- Ridge --North-- Cape
    ///                   |
    ///                 East
    ///                   |
    ///                 Tide
    ///
    ///   Islet --Orphan-- Jetty        (joined to nothing)
    /// ```
    const MAP: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0,0,24,16">
      <g>
        <path d="M0 0l3 0l0 3l-3 0z" fill="white" stroke="rgb(223,223,247)" stroke-width="0.05"/>
        <text>Key to lines</text>
        <path d="M0.5 0.5l1 0" fill="none" stroke="hsl(0,100%,50%)" stroke-width="0.2"/>
        <text>North</text>
        <path d="M0.5 1l1 0" fill="none" stroke="hsl(210,100%,50%)" stroke-width="0.2"/>
        <text>East</text>
        <path d="M0.5 1.5l1 0" fill="none" stroke="hsl(40,100%,50%)" stroke-width="0.2"/>
        <text>Orphan</text>
      </g>
      <g>
        <path d="M1.5 1.5L1.5 2.5L1.5 3.5" fill="none" stroke="hsl(0,100%,50%)" stroke-width="0.2"/>
        <path d="M1.5 3.5L1.5 4.5L1.5 5.5" fill="none" stroke="hsl(0,100%,50%)" stroke-width="0.2"/>
        <path d="M1.5 5.5L1.5 8.5L1.5 11.5" fill="none" stroke="hsl(0,100%,50%)" stroke-width="0.2"/>
        <path d="M1.5 3.5L2.5 3.5L3.5 3.5" fill="none" stroke="hsl(210,100%,50%)" stroke-width="0.2"/>
        <path d="M6.5 13.5L7.5 13.5L8.5 13.5" fill="none" stroke="hsl(40,100%,50%)" stroke-width="0.2"/>
      </g>
      <g>
        <ellipse cx="1.5" cy="1.5" rx="0.35" ry="0.35" fill="white" stroke="black" stroke-width="0.16"/>
        <g><text>Pier</text></g>
        <ellipse cx="1.5" cy="3.5" rx="0.35" ry="0.35" fill="white" stroke="black" stroke-width="0.16"/>
        <g><text>Quay</text></g>
        <ellipse cx="1.5" cy="5.5" rx="0.35" ry="0.35" fill="white" stroke="black" stroke-width="0.16"/>
        <g><text>Ridge</text></g>
        <path d="M1.5 11.275l0 0.45" stroke="hsl(0,100%,50%)" stroke-width="0.2"/>
        <g><text>Cape</text></g>
        <path d="M3.5 3.275l0 0.45" stroke="hsl(210,100%,50%)" stroke-width="0.2"/>
        <g><text>Tide</text></g>
        <path d="M6.5 13.275l0 0.45" stroke="hsl(40,100%,50%)" stroke-width="0.2"/>
        <g><text>Islet</text></g>
        <path d="M8.5 13.275l0 0.45" stroke="hsl(40,100%,50%)" stroke-width="0.2"/>
        <g><text>Jetty</text></g>
      </g>
    </svg>"##;

    fn net() -> Network {
        parse(MAP).expect("fixture parses")
    }

    /// A kilometre to the map unit, so the times follow the drawing.
    fn flat(net: &Network) -> Timing<'_> {
        let straight = Scale::from_points((1.5, 5.5), vec![(40.0, 40.0)]);
        Timing::new(net, &straight, &Services::for_network(net))
    }

    fn at(net: &Network, name: &str) -> StationId {
        net.station_named(name).unwrap_or_else(|| panic!("no station {name}"))
    }

    #[test]
    fn the_origin_is_no_distance_from_itself_and_its_neighbours_are_one_ride() {
        let net = net();
        let timing = flat(&net);
        let spread = Spread::from(&timing, at(&net, "Quay"));
        assert_eq!(spread.origin(), at(&net, "Quay"));
        assert_eq!(spread.seconds(at(&net, "Quay")), Some(0.0));
        assert_eq!(spread.start(&net), (1.5, 3.5));

        // Pier is one segment up the North line, with no wait for the first
        // service and no stand at an intermediate stop.
        let pier = spread.seconds(at(&net, "Pier")).expect("Pier is reachable");
        assert!((pier - timing.edge(crate::EdgeId(0))).abs() < 1e-9, "{pier} s");
        // Quay sits between Pier and Ridge, one equal hop either way, so the
        // two cost the same: an isochrone is a shape, not a list.
        let ridge = spread.seconds(at(&net, "Ridge")).expect("Ridge is reachable");
        assert!((pier - ridge).abs() < 1e-9, "{pier} s one way, {ridge} s the other");
        // And the long hop out to Cape costs more than either.
        let cape = spread.seconds(at(&net, "Cape")).expect("Cape is reachable");
        assert!(cape > ridge * 2.0, "{cape} s against {ridge} s");
    }

    #[test]
    fn what_no_line_reaches_has_no_time_at_all() {
        let net = net();
        let timing = flat(&net);
        let spread = Spread::from(&timing, at(&net, "Pier"));
        assert_eq!(spread.seconds(at(&net, "Islet")), None, "Orphan joins nothing");
        assert_eq!(spread.seconds(at(&net, "Jetty")), None);
        assert_eq!(spread.reached(), net.stations.len() - 2);
        // And a station id from nowhere is absent rather than a panic.
        assert_eq!(spread.seconds(StationId(9_999)), None);
    }

    #[test]
    fn an_isochrone_says_what_the_itinerary_says() {
        // The whole reason this shares a cost model with the planner: a dot
        // sitting inside the half-hour ring has to be a dot the itinerary
        // would get to in half an hour.
        let net = net();
        let timing = flat(&net);
        let from = at(&net, "Pier");
        let spread = Spread::from(&timing, from);
        for station in &net.stations {
            let Some(mine) = spread.seconds(station.id) else {
                continue;
            };
            if station.id == from {
                continue;
            }
            let journey = fastest(&timing, from, station.id).expect("reachable, so routable");
            assert!(
                (mine - journey.seconds).abs() < 1e-9,
                "{}: isochrone {mine} s, itinerary {} s",
                net.label(station.id),
                journey.seconds,
            );
        }
    }

    #[test]
    fn a_limit_takes_in_more_as_it_grows() {
        let net = net();
        let timing = flat(&net);
        let spread = Spread::from(&timing, at(&net, "Pier"));

        let mut had = 0;
        for minutes in [1.0, 5.0, 10.0, 30.0, 90.0] {
            let stops = spread.stops(&net, minutes * 60.0);
            assert!(stops.len() >= had, "{} at {minutes} min after {had}", stops.len());
            had = stops.len();
            // Soonest first, nothing past the limit, and the origin is in it.
            for pair in stops.windows(2) {
                assert!(pair[0].seconds <= pair[1].seconds);
            }
            assert!(stops.iter().all(|s| s.seconds <= minutes * 60.0 + 1e-9));
            assert_eq!(stops[0].station, at(&net, "Pier"));
            assert_eq!(stops[0].seconds, 0.0);
            assert_eq!(spread.counts(minutes * 60.0).iter().sum::<usize>(), stops.len());
        }
        assert_eq!(had, spread.reached(), "an hour and a half covers this fixture");
    }

    #[test]
    fn the_bands_split_the_limit_evenly_and_keep_the_limit_itself() {
        assert_eq!(band(0.0, 600.0), 0);
        assert_eq!(band(149.0, 600.0), 0);
        assert_eq!(band(150.0, 600.0), 1);
        assert_eq!(band(449.0, 600.0), 2);
        assert_eq!(band(599.0, 600.0), BANDS - 1);
        assert_eq!(band(600.0, 600.0), BANDS - 1, "the edge belongs to the last band");
        // Nonsense picks the nearest band rather than indexing off the ramp.
        assert_eq!(band(-5.0, 600.0), 0);
        assert_eq!(band(10.0, 0.0), 0);
        assert_eq!(band(f64::NAN, 600.0), 0);
        assert_eq!(band(10.0, f64::NAN), 0);
    }

    #[test]
    fn the_frontier_is_cut_where_the_time_runs_out() {
        let net = net();
        let timing = flat(&net);
        let spread = Spread::from(&timing, at(&net, "Pier"));

        // Ridge to Cape is a 6 unit hop, far longer than the others. A limit
        // just past Ridge must leave a stub of it, not the whole thing.
        let ridge = spread.seconds(at(&net, "Ridge")).expect("Ridge is reachable");
        let cape = spread.seconds(at(&net, "Cape")).expect("Cape is reachable");
        let limit = ridge + (cape - ridge) * 0.4;

        let whole: f64 = net
            .edges
            .iter()
            .filter(|e| {
                (e.a == at(&net, "Ridge") && e.b == at(&net, "Cape"))
                    || (e.b == at(&net, "Ridge") && e.a == at(&net, "Cape"))
            })
            .map(|e| e.length)
            .sum();
        assert!(whole > 5.0, "the fixture draws a long hop there");

        let drawn = spread.fringe(&timing, limit);
        assert!(!drawn.is_empty());
        for piece in &drawn {
            assert!(piece.points.len() >= 2, "a stub of one point draws nothing");
            assert!(piece.seconds <= limit + 1e-9, "drawn from a reachable end");
            assert_eq!(piece.band, band(piece.seconds, limit));
        }
        // Somewhere out there is a piece shorter than the track it lies on.
        let stub = drawn
            .iter()
            .map(|p| polyline_length(&p.points))
            .find(|len| *len > 0.0 && *len < whole * 0.9)
            .expect("a cut piece");
        assert!(stub < whole, "{stub} against {whole}");

        // Cape is outside the limit, so nothing is drawn right up to it.
        assert!(spread.stops(&net, limit).iter().all(|s| s.station != at(&net, "Cape")));
        let tip = (1.5, 11.5);
        for piece in &drawn {
            let ends = [piece.points[0], piece.points[piece.points.len() - 1]];
            assert!(
                ends.iter().all(|(x, y)| (x - tip.0).hypot(y - tip.1) > 0.5),
                "something reaches Cape at {limit} s",
            );
        }
    }

    #[test]
    fn a_change_is_paid_for_before_the_frontier_moves_on() {
        // The bug this fixture is here for. Quay is a junction: arriving from
        // Pier on North, carrying on to Tide means changing onto East, and the
        // walk and the wait come before the train moves at all. Measuring the
        // frontier from the arrival instead drew the East line out of Quay for
        // free and reached a station the limit does not include.
        let net = net();
        let timing = flat(&net);
        let spread = Spread::from(&timing, at(&net, "Pier"));

        let east = net.lines.iter().find(|l| l.name == "East").expect("the East line").id;
        let hop = net
            .edges
            .iter()
            .find(|e| e.line == east && (e.a == at(&net, "Quay") || e.b == at(&net, "Quay")))
            .expect("Quay to Tide");

        let quay = spread.seconds(at(&net, "Quay")).expect("reachable");
        let out = spread.departure(&timing, at(&net, "Quay"), hop.id).expect("boardable");
        assert!(
            (out - quay - timing.boarding(east)).abs() < 1e-9,
            "leaving on East costs the arrival plus the change: {out} s against {quay} s",
        );

        // A limit that clears the ride but not the change.
        let limit = quay + timing.edge(hop.id) + 1.0;
        assert!(out > limit, "the fixture has to pose the question: {out} s against {limit} s");
        assert!(
            spread.seconds(at(&net, "Tide")).expect("reachable") > limit,
            "and Tide has to be outside it",
        );

        // So nothing is drawn along East out of Quay, and no dot appears.
        let tide = net.station(at(&net, "Tide")).cell.centre();
        for piece in spread.fringe(&timing, limit) {
            for (x, y) in &piece.points {
                assert!(
                    (x - tide.0).hypot(y - tide.1) > 0.5,
                    "drew to Tide at {limit} s, which takes {:?} s",
                    spread.seconds(at(&net, "Tide")),
                );
            }
        }
        assert!(spread.stops(&net, limit).iter().all(|s| s.station != at(&net, "Tide")));

        // Open it up by the change and the track appears.
        let paid = spread.fringe(&timing, limit + timing.boarding(east));
        assert!(
            paid.iter().any(|piece| piece.points.len() >= 2
                && piece.points.iter().any(|(x, y)| {
                    (x - 1.5).hypot(y - 3.5) < 1.6 && *x > 1.6
                })),
            "the East line should be drawn out of Quay once the change is covered",
        );
    }

    #[test]
    fn staying_aboard_costs_a_stand_and_no_more() {
        // The other half of a departure: riding straight through a stop is a
        // stand, not a change, and the frontier has to know the difference or
        // every through station would cost a wait.
        let net = net();
        let timing = flat(&net);
        let spread = Spread::from(&timing, at(&net, "Pier"));
        let quay = spread.seconds(at(&net, "Quay")).expect("reachable");

        let north = net.lines.iter().find(|l| l.name == "North").expect("the North line").id;
        let onward = net
            .edges
            .iter()
            .find(|e| e.line == north && (e.a == at(&net, "Ridge") || e.b == at(&net, "Ridge")))
            .filter(|e| e.a == at(&net, "Quay") || e.b == at(&net, "Quay"))
            .expect("Quay to Ridge");
        let out = spread.departure(&timing, at(&net, "Quay"), onward.id).expect("boardable");
        assert!(
            (out - quay - timing.dwell).abs() < 1e-9,
            "{out} s should be {quay} s and a {} s stand",
            timing.dwell,
        );
        // And the origin itself departs at once, on any line it has.
        for edge in &net.station(at(&net, "Pier")).edges {
            assert_eq!(spread.departure(&timing, at(&net, "Pier"), *edge), Some(0.0));
        }
        // A station nothing reaches cannot be departed from.
        let orphan = net.station(at(&net, "Islet")).edges[0];
        assert_eq!(spread.departure(&timing, at(&net, "Islet"), orphan), None);
    }

    #[test]
    fn a_generous_limit_draws_every_stretch_whole() {
        let net = net();
        let timing = flat(&net);
        let spread = Spread::from(&timing, at(&net, "Pier"));
        let drawn = spread.fringe(&timing, 10.0 * 3600.0);
        // Every rideable segment the search can reach, each in one piece.
        let rideable = net.edges.iter().filter(|e| timing.rides(e.line)).count();
        assert_eq!(drawn.len(), rideable - 1, "all but the orphan line's own hop");
        for piece in &drawn {
            assert_eq!(piece.band, 0, "it is all nearby at that limit");
        }
        let total: f64 = drawn.iter().map(|p| polyline_length(&p.points)).sum();
        let track: f64 = net
            .edges
            .iter()
            .filter(|e| timing.rides(e.line) && spread.seconds(e.a).is_some())
            .map(|e| e.length)
            .sum();
        assert!((total - track).abs() < 1e-6, "{total} drawn against {track} of track");
    }

    #[test]
    fn nothing_is_drawn_from_a_station_the_limit_does_not_reach() {
        let net = net();
        let timing = flat(&net);
        let spread = Spread::from(&timing, at(&net, "Pier"));
        assert!(spread.fringe(&timing, 0.0).is_empty(), "no time, no isochrone");
        assert!(spread.stops(&net, 0.0).len() == 1, "except where you are standing");
        // And the orphan line is never drawn, however long the limit.
        let islet = net.station(at(&net, "Islet")).cell.centre();
        for piece in spread.fringe(&timing, 10.0 * 3600.0) {
            for (x, y) in &piece.points {
                assert!((x - islet.0).hypot(y - islet.1) > 1.0, "drew the orphan line");
            }
        }
    }

    #[test]
    fn switching_a_category_off_shrinks_the_isochrone() {
        let net = net();
        let open = flat(&net);
        let wide = Spread::from(&open, at(&net, "Quay"));
        assert!(wide.seconds(at(&net, "Tide")).is_some());

        // Put the East line in a category of its own and switch it off.
        let mut services = Services::for_network(&net);
        let east = net.lines.iter().find(|l| l.name == "East").expect("the East line").id;
        let apart = services.add("Not today");
        services.assign(east, apart);
        services.set_allowed(apart, false);
        let shut = Timing::new(&net, &Scale::from_points((1.5, 5.5), vec![(40.0, 40.0)]), &services);
        let narrow = Spread::from(&shut, at(&net, "Quay"));

        assert_eq!(narrow.seconds(at(&net, "Tide")), None, "Tide is only on East");
        assert!(narrow.reached() < wide.reached());
        assert_eq!(narrow.seconds(at(&net, "Pier")), wide.seconds(at(&net, "Pier")));
        // Nor is its track drawn.
        let tide = net.station(at(&net, "Tide")).cell.centre();
        for piece in narrow.fringe(&shut, 10.0 * 3600.0) {
            for (x, y) in &piece.points {
                assert!((x - tide.0).hypot(y - tide.1) > 0.5, "drew a switched-off line");
            }
        }
    }

    #[test]
    fn the_layer_is_drawn_so_the_fade_leaves_it_alone() {
        let net = net();
        let timing = flat(&net);
        let spread = Spread::from(&timing, at(&net, "Pier"));
        // Chosen so that the fixture spans several bands.
        let limit = 300.0;
        let svg = layer(
            &spread.fringe(&timing, limit),
            &spread.stops(&net, limit),
            spread.start(&net),
        );

        // Every stroke and dot carries the class the fade rules step around,
        // or the isochrone would grey out the moment it was drawn.
        assert!(svg.contains(&format!("class=\"{LEG_CLASS}\"")));
        assert_eq!(svg.matches("class=").count(), svg.matches('<').count());
        // Drawn in the ramp and nothing else: no line's own colour leaks in,
        // which is what would make a band mean two different things.
        let bands = BAND_INK.iter().filter(|ink| svg.contains(**ink)).count();
        assert!(bands >= 3, "only {bands} band(s) drawn across this fixture");
        for colour in svg.split("stroke=\"").skip(1).chain(svg.split("fill=\"").skip(1)) {
            let used = &colour[..colour.find('"').expect("a closing quote")];
            assert!(
                used == "none" || used == "#fff" || used == "#23232b" || BAND_INK.contains(&used),
                "{used} is not part of the ramp",
            );
        }
        assert!(svg.contains("cx=\"1.5000\" cy=\"1.5000\""), "a ring at Pier");
        assert!(!svg.contains("NaN"));
        // Casings before the ink they sit under.
        let first_ink = svg.find(BAND_INK[0]).expect("some band");
        assert!(svg.find("#23232b").expect("a casing") < first_ink);
    }

    #[test]
    fn the_minutes_are_stamped_beside_the_stations_they_belong_to() {
        let net = net();
        let timing = flat(&net);
        let spread = Spread::from(&timing, at(&net, "Pier"));
        let limit = 600.0;
        let stops = spread.stops(&net, limit);
        let svg = stamp(&stops);

        assert_eq!(svg.matches("<text").count(), stops.len(), "one per station reached");
        // Where you are standing is nought minutes away, above its own dot.
        assert!(svg.contains(&format!(
            "x=\"1.5000\" y=\"{:.4}\"", 1.5 - STAMP_OFFSET,
        )));
        assert!(svg.contains(">0</text>"));
        // Quay is two minutes up the line at this scale, to the nearest minute.
        let quay = spread.seconds(at(&net, "Quay")).expect("reachable");
        assert!(svg.contains(&format!(">{}</text>", (quay / 60.0).round() as u64)));
        // Marked like everything else drawn over the map, so the fade leaves
        // it alone, and with no raw text from the map in it to escape.
        assert_eq!(svg.matches(&format!("class=\"{LEG_CLASS}\"")).count(), stops.len());
        assert!(!svg.contains("NaN"));
        assert!(stamp(&[]).is_empty());
    }

    #[test]
    fn a_station_can_be_pointed_at_rather_than_named() {
        // How an isochrone is started: a click lands somewhere near a marker,
        // and how near counts depends on how far out the map is zoomed.
        let net = net();
        let quay = at(&net, "Quay");
        assert_eq!(net.station_near((1.5, 3.5), 0.1), Some(quay), "right on it");
        assert_eq!(net.station_near((1.7, 3.3), 0.5), Some(quay), "near enough");
        assert_eq!(net.station_near((1.7, 3.3), 0.05), None, "not that near");
        // Between two stations, the nearer one wins. Pier is at 1.5, Quay at
        // 3.5, so the halfway line is 2.5.
        assert_eq!(net.station_near((1.5, 2.6), 2.0), Some(quay));
        assert_eq!(net.station_near((1.5, 2.4), 2.0), Some(at(&net, "Pier")));
        // Empty paper stays empty paper.
        assert_eq!(net.station_near((40.0, 40.0), 1.0), None);
        // And nonsense picks nothing rather than the first station on the map.
        assert_eq!(net.station_near((f64::NAN, 1.0), 1.0), None);
        assert_eq!(net.station_near((1.5, 3.5), f64::NAN), None);
        assert_eq!(net.station_near((1.5, 3.5), -1.0), None);
    }

    #[test]
    fn a_cut_runs_along_the_track_it_cuts() {
        let line = vec![(0.0, 0.0), (2.0, 0.0), (2.0, 2.0)];
        assert_eq!(cut(&line, 1.0), line, "all of it is all of it");
        assert_eq!(cut(&line, 2.0), line, "and so is more than all of it");
        assert_eq!(cut(&line, 0.25), vec![(0.0, 0.0), (1.0, 0.0)]);
        assert_eq!(cut(&line, 0.5), vec![(0.0, 0.0), (2.0, 0.0)], "right on a corner");
        assert_eq!(cut(&line, 0.75), vec![(0.0, 0.0), (2.0, 0.0), (2.0, 1.0)], "past it");
        assert_eq!(cut(&line, 0.0), vec![(0.0, 0.0), (0.0, 0.0)], "a nub, not a gap");
        // Lengths come out as asked for.
        for share in [0.1, 0.33, 0.6, 0.9] {
            let piece = cut(&line, share);
            assert!(
                (polyline_length(&piece) - 4.0 * share).abs() < 1e-9,
                "{share} of 4 units came out {}",
                polyline_length(&piece),
            );
        }
        // Junk in, the whole thing out, rather than a panic.
        assert_eq!(cut(&[(1.0, 1.0)], 0.5), vec![(1.0, 1.0)]);
        assert_eq!(cut(&[], 0.5), Vec::new());
    }
}
