//! A first journey planner over the parsed graph.
//!
//! Provisional by design: there are no timetables, acceleration curves or
//! station-spacing effects yet, so "fastest" means the least drawn track plus
//! flat penalties for stopping and for changing lines. Grid units stand in for
//! minutes. When the real time model arrives only [`cost_of`] needs to change —
//! the search itself is already shaped for it.

use crate::network::{EdgeId, LineId, Network, StationId};
use std::cmp::{Ordering, Reverse};
use std::collections::{BTreeMap, BinaryHeap};
use std::fmt;

/// Charged once per stop reached, standing in for braking, dwelling and
/// accelerating away again.
pub const DWELL: f64 = 0.6;
/// Charged for changing lines: walking the interchange and waiting for the
/// next service. Deliberately heavy, since a change costs a passenger far more
/// than the ~3 grid units of a typical hop.
pub const TRANSFER: f64 = 8.0;

/// One continuous ride on a single line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Leg {
    pub line: LineId,
    /// Boarding station first, alighting station last.
    pub stops: Vec<StationId>,
    pub edges: Vec<EdgeId>,
}

impl Leg {
    /// Stations travelled through, counting the one alighted at.
    pub fn stations(&self) -> usize {
        self.stops.len().saturating_sub(1)
    }
    pub fn board(&self) -> StationId {
        self.stops[0]
    }
    pub fn alight(&self) -> StationId {
        self.stops[self.stops.len() - 1]
    }

    /// This leg's segments, each flipped if need be so it runs in travel order.
    ///
    /// Tennessine draws a segment in whichever direction it pleases, so nothing
    /// that walks a leg can take the stored order on trust.
    fn oriented<'a>(&'a self, net: &'a Network) -> impl Iterator<Item = Vec<(f64, f64)>> + 'a {
        self.edges.iter().enumerate().map(move |(i, id)| {
            let edge = net.edge(*id);
            let mut points = edge.polyline.clone();
            if self.stops.get(i) == Some(&edge.b) {
                points.reverse();
            }
            points
        })
    }

    /// The leg's drawn geometry, one polyline per stretch of track between two
    /// stations, in travel order.
    ///
    /// [`Self::polyline`] is these joined end to end. They are worth having
    /// apart when each stretch is to be measured or labelled on its own.
    pub fn segments(&self, net: &Network) -> Vec<Vec<(f64, f64)>> {
        self.oriented(net).collect()
    }

    /// The leg's drawn geometry as one polyline in travel order.
    ///
    /// Joining the segments matters for drawing: separate strokes meeting at a
    /// station leave a notch at every corner, one stroke does not.
    pub fn polyline(&self, net: &Network) -> Vec<(f64, f64)> {
        let mut out: Vec<(f64, f64)> = Vec::new();
        for points in self.oriented(net) {
            match out.is_empty() {
                true => out = points,
                // the station the two segments share is already there
                false => out.extend(points.into_iter().skip(1)),
            }
        }
        out
    }

    /// Where this leg's stops sit **as drawn**, in travel order.
    ///
    /// Not the same as where the stations sit: parallel lines in a corridor are
    /// offset from the cell centre by multiples of 0.095, so a marker placed at
    /// the centre floats beside the route instead of sitting on it.
    pub fn stop_points(&self, net: &Network) -> Vec<(f64, f64)> {
        let mut out: Vec<(f64, f64)> = Vec::new();
        for points in self.oriented(net) {
            if let (Some(first), Some(last)) = (points.first(), points.last()) {
                if out.is_empty() {
                    out.push(*first);
                }
                out.push(*last);
            }
        }
        out
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Journey {
    pub legs: Vec<Leg>,
    /// In the notional units described above, not minutes.
    pub cost: f64,
}

impl Journey {
    pub fn stations(&self) -> usize {
        self.legs.iter().map(Leg::stations).sum()
    }
    pub fn transfers(&self) -> usize {
        self.legs.len().saturating_sub(1)
    }
    /// Where each change happens, in order.
    pub fn transfer_points(&self) -> Vec<StationId> {
        self.legs.windows(2).map(|w| w[0].alight()).collect()
    }
    pub fn distance(&self, net: &Network) -> f64 {
        self.legs
            .iter()
            .flat_map(|l| l.edges.iter())
            .map(|e| net.edge(*e).length)
            .sum()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteError {
    SameStation,
    /// No chain of routable lines joins the two.
    Unreachable,
}

impl fmt::Display for RouteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SameStation => write!(f, "pick two different stations"),
            Self::Unreachable => write!(f, "no routable line connects these two stations"),
        }
    }
}

impl std::error::Error for RouteError {}

/// What riding `edge` costs, having arrived on `arrived_on`.
fn cost_of(net: &Network, edge: EdgeId, arrived_on: Option<LineId>) -> f64 {
    let e = net.edge(edge);
    let change = matches!(arrived_on, Some(line) if line != e.line);
    e.length + DWELL + if change { TRANSFER } else { 0.0 }
}

/// The cheapest journey between two stations, over routable lines only.
///
/// The search runs over (station, line aboard) rather than plain stations, so
/// that staying put on a line can be preferred to changing at the same spot.
pub fn fastest(net: &Network, from: StationId, to: StationId) -> Result<Journey, RouteError> {
    if from == to {
        return Err(RouteError::SameStation);
    }

    type State = (StationId, LineId);
    let mut best: BTreeMap<State, f64> = BTreeMap::new();
    let mut came: BTreeMap<State, (StationId, Option<LineId>, EdgeId)> = BTreeMap::new();
    let mut queue: BinaryHeap<Reverse<(Cost, State)>> = BinaryHeap::new();

    for (next, edge) in net.neighbours(from) {
        let state = (next, edge.line);
        let cost = cost_of(net, edge.id, None);
        if best.get(&state).is_none_or(|had| cost < *had) {
            best.insert(state, cost);
            came.insert(state, (from, None, edge.id));
            queue.push(Reverse((Cost(cost), state)));
        }
    }

    let mut arrival: Option<(State, f64)> = None;
    while let Some(Reverse((Cost(cost), state))) = queue.pop() {
        if cost > best.get(&state).copied().unwrap_or(f64::INFINITY) {
            continue; // a cheaper way here was already settled
        }
        if state.0 == to {
            arrival = Some((state, cost));
            break;
        }
        let (at, aboard) = state;
        for (next, edge) in net.neighbours(at) {
            let onward = (next, edge.line);
            let total = cost + cost_of(net, edge.id, Some(aboard));
            if best.get(&onward).is_none_or(|had| total < *had) {
                best.insert(onward, total);
                came.insert(onward, (at, Some(aboard), edge.id));
                queue.push(Reverse((Cost(total), onward)));
            }
        }
    }

    let (mut state, cost) = arrival.ok_or(RouteError::Unreachable)?;

    // Walk the predecessors back to the origin, then ride forwards again.
    let mut ridden = Vec::new();
    loop {
        let (previous, aboard, via) = came[&state];
        ridden.push(via);
        match aboard {
            Some(line) => state = (previous, line),
            None => break,
        }
    }
    ridden.reverse();

    let mut legs: Vec<Leg> = Vec::new();
    let mut at = from;
    for id in ridden {
        let edge = net.edge(id);
        let next = edge.other(at);
        match legs.last_mut() {
            Some(leg) if leg.line == edge.line => {
                leg.stops.push(next);
                leg.edges.push(id);
            }
            _ => legs.push(Leg { line: edge.line, stops: vec![at, next], edges: vec![id] }),
        }
        at = next;
    }

    Ok(Journey { legs, cost })
}

/// f64 ordered for the heap. Costs here are always finite.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Cost(f64);
impl Eq for Cost {}
impl Ord for Cost {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.partial_cmp(&other.0).unwrap_or(Ordering::Equal)
    }
}
impl PartialOrd for Cost {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;

    /// Two lines crossing at Quay, plus a long way round that the planner
    /// should reject even though it needs no change.
    ///
    /// ```text
    ///   Pier --North-- Quay --North-- Ridge
    ///     |              |
    ///     |            East
    ///  Scenic            |
    ///     |            Tide -- East -- Union
    ///   Cape ----------- Scenic -----/
    /// ```
    const MAP: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0,0,24,12">
      <g>
        <path d="M0 0l3 0l0 3l-3 0z" fill="white" stroke="rgb(223,223,247)" stroke-width="0.05"/>
        <text>Key to lines</text>
        <path d="M0.5 0.5l1 0" fill="none" stroke="hsl(0,100%,50%)" stroke-width="0.2"/>
        <text>North</text>
        <path d="M0.5 1l1 0" fill="none" stroke="hsl(210,100%,50%)" stroke-width="0.2"/>
        <text>East</text>
        <path d="M0.5 1.5l1 0" fill="none" stroke="hsl(120,100%,30%)" stroke-width="0.2"/>
        <text>Scenic</text>
        <path d="M0.5 2l1 0" fill="none" stroke="hsl(40,100%,50%)" stroke-width="0.2"/>
        <text>Orphan</text>
      </g>
      <g>
        <path d="M1.5 1.5L1.5 2.5L1.5 3.5" fill="none" stroke="hsl(0,100%,50%)" stroke-width="0.2"/>
        <path d="M1.5 3.5L1.5 4.5L1.5 5.5" fill="none" stroke="hsl(0,100%,50%)" stroke-width="0.2"/>
        <path d="M1.5 3.5L2.5 3.5L3.5 3.5" fill="none" stroke="hsl(210,100%,50%)" stroke-width="0.2"/>
        <path d="M3.5 3.5L4.5 3.5L5.5 3.5" fill="none" stroke="hsl(210,100%,50%)" stroke-width="0.2"/>
        <path d="M1.5 1.5L7.5 1.5L13.5 1.5" fill="none" stroke="hsl(120,100%,30%)" stroke-width="0.2"/>
        <path d="M13.5 1.5L9.5 2.5L5.5 3.5" fill="none" stroke="hsl(120,100%,30%)" stroke-width="0.2"/>
        <path d="M1.5 9.5L2.5 9.5L3.5 9.5" fill="none" stroke="hsl(40,100%,50%)" stroke-width="0.2"/>
      </g>
      <g>
        <ellipse cx="1.5" cy="1.5" rx="0.35" ry="0.35" fill="white" stroke="black" stroke-width="0.16"/>
        <g><text>Pier</text></g>
        <ellipse cx="1.5" cy="3.5" rx="0.35" ry="0.35" fill="white" stroke="black" stroke-width="0.16"/>
        <g><text>Quay</text></g>
        <path d="M1.5 5.275l0 0.45" stroke="hsl(0,100%,50%)" stroke-width="0.2"/>
        <g><text>Ridge</text></g>
        <path d="M3.5 3.275l0 0.45" stroke="hsl(210,100%,50%)" stroke-width="0.2"/>
        <g><text>Tide</text></g>
        <ellipse cx="5.5" cy="3.5" rx="0.35" ry="0.35" fill="white" stroke="black" stroke-width="0.16"/>
        <g><text>Union</text></g>
        <path d="M13.5 1.275l0 0.45" stroke="hsl(120,100%,30%)" stroke-width="0.2"/>
        <g><text>Cape</text></g>
        <path d="M1.5 9.275l0 0.45" stroke="hsl(40,100%,50%)" stroke-width="0.2"/>
        <g><text>Islet</text></g>
        <path d="M3.5 9.275l0 0.45" stroke="hsl(40,100%,50%)" stroke-width="0.2"/>
        <g><text>Jetty</text></g>
      </g>
    </svg>"##;

    fn net() -> Network {
        parse(MAP).expect("fixture parses")
    }

    fn at(net: &Network, name: &str) -> StationId {
        *net.search_index().get(name).unwrap_or_else(|| panic!("no station {name}")).first().unwrap()
    }

    fn described(net: &Network, j: &Journey) -> Vec<String> {
        let mut out = Vec::new();
        for (i, leg) in j.legs.iter().enumerate() {
            out.push(format!("{} {}", net.line(leg.line).name, leg.stations()));
            if i + 1 < j.legs.len() {
                out.push(format!("transfer at {}", net.label(leg.alight())));
            }
        }
        out
    }

    #[test]
    fn a_single_line_journey_has_one_leg_and_no_transfer() {
        let net = net();
        let j = fastest(&net, at(&net, "Pier"), at(&net, "Ridge")).unwrap();
        assert_eq!(described(&net, &j), ["North 2"]);
        assert_eq!(j.transfers(), 0);
        assert_eq!(j.stations(), 2);
        assert!(j.transfer_points().is_empty());
    }

    #[test]
    fn changing_lines_is_reported_at_the_right_station() {
        let net = net();
        let j = fastest(&net, at(&net, "Pier"), at(&net, "Union")).unwrap();
        assert_eq!(described(&net, &j), ["North 1", "transfer at Quay", "East 2"]);
        assert_eq!(j.transfers(), 1);
        assert_eq!(j.stations(), 3);
        assert_eq!(j.transfer_points(), vec![at(&net, "Quay")]);
    }

    #[test]
    fn a_change_beats_a_much_longer_one_seat_ride() {
        let net = net();
        let j = fastest(&net, at(&net, "Pier"), at(&net, "Union")).unwrap();
        // Scenic runs Pier -> Cape -> Union with no change at all, but ~20 units
        assert!(
            j.legs.iter().all(|l| net.line(l.line).name != "Scenic"),
            "took the scenic route: {:?}", described(&net, &j),
        );
        assert!(j.cost < 20.0, "cost {} should beat the ~21 unit scenic ride", j.cost);
    }

    #[test]
    fn one_seat_wins_when_the_distance_is_comparable() {
        let net = net();
        // Pier -> Cape is Scenic all the way; going North then East then Scenic
        // back out would be absurd, and the transfer charge guarantees it loses
        let j = fastest(&net, at(&net, "Pier"), at(&net, "Cape")).unwrap();
        assert_eq!(described(&net, &j), ["Scenic 1"]);
    }

    #[test]
    fn journeys_are_symmetric() {
        let net = net();
        let there = fastest(&net, at(&net, "Pier"), at(&net, "Union")).unwrap();
        let back = fastest(&net, at(&net, "Union"), at(&net, "Pier")).unwrap();
        assert!((there.cost - back.cost).abs() < 1e-9, "{} vs {}", there.cost, back.cost);
        assert_eq!(there.transfers(), back.transfers());
    }

    #[test]
    fn legs_join_end_to_end() {
        let net = net();
        let j = fastest(&net, at(&net, "Ridge"), at(&net, "Union")).unwrap();
        assert_eq!(j.legs[0].board(), at(&net, "Ridge"));
        assert_eq!(j.legs[j.legs.len() - 1].alight(), at(&net, "Union"));
        for pair in j.legs.windows(2) {
            assert_eq!(pair[0].alight(), pair[1].board(), "a change happens in one place");
        }
        for leg in &j.legs {
            assert_eq!(leg.edges.len(), leg.stations());
            for e in &leg.edges {
                assert_eq!(net.edge(*e).line, leg.line, "a leg stays on its line");
            }
        }
    }

    #[test]
    fn a_leg_breaks_into_the_stretches_it_is_joined_from() {
        let net = net();
        let j = fastest(&net, at(&net, "Pier"), at(&net, "Union")).unwrap();
        for leg in &j.legs {
            let stretches = leg.segments(&net);
            assert_eq!(stretches.len(), leg.edges.len(), "one per segment ridden");

            // Each stretch starts where the one before it ended, which is the
            // station between them, and together they are the leg's polyline.
            let mut joined = Vec::new();
            for stretch in &stretches {
                assert!(stretch.len() >= 2, "a stretch of track has length");
                match joined.last() {
                    None => joined.extend(stretch.iter().copied()),
                    Some(end) => {
                        assert_eq!(end, &stretch[0], "stretches meet at their station");
                        joined.extend(stretch.iter().skip(1).copied());
                    }
                }
            }
            assert_eq!(joined, leg.polyline(&net));
        }
    }

    #[test]
    fn says_so_when_there_is_no_route() {
        let net = net();
        let pier = at(&net, "Pier");
        assert_eq!(fastest(&net, pier, pier), Err(RouteError::SameStation));
        // Orphan is a perfectly good line, just not joined to anything
        assert_eq!(fastest(&net, pier, at(&net, "Islet")), Err(RouteError::Unreachable));
        assert!(fastest(&net, at(&net, "Islet"), at(&net, "Jetty")).is_ok());
    }
}
