//! The journey planner.
//!
//! "Fastest" means fastest: the search runs on the seconds that
//! [`Timing`] gives it — real distances through the user's scale, ridden at
//! the speed and acceleration of each line's service category, plus a stand at
//! every stop and a penalty for every change. What the planner minimises and
//! what the app displays are the same number, worked out in one place.
//!
//! The search is also free to be told what it may not use: a service category
//! switched off in the routing panel takes its lines out of the graph
//! altogether, which is how "how would I get there without the commuter
//! rail?" is asked.
//!
//! [`fastest`] answers with one journey. [`options`] answers with the two or
//! three that are worth putting side by side — the quickest, the one with the
//! fewest changes, the one that goes the other way round — each labelled with
//! the question it is the best answer to.
//!
//! What is still missing lives in [`crate::timing`]: anything that varies by
//! time of day.

use crate::network::{EdgeId, LineId, Network, StationId};
use crate::timing::Timing;
use std::cmp::{Ordering, Reverse};
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};
use std::fmt;

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
    /// Seconds from boarding the first service to stepping off the last: the
    /// number the search minimised, and the one to show.
    pub seconds: f64,
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

/// How many journeys are worth putting side by side. Three is what fits a
/// glance; a fourth card is read by nobody.
pub const OPTIONS: usize = 3;
/// How much slower than the quickest an option may be before it stops being
/// another way to go.
pub const SPREAD: f64 = 1.5;
/// How much track two journeys may share before they are the same route with
/// two labels on it.
const SAME: f64 = 0.85;

/// What makes a journey worth offering alongside the others.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Merit {
    /// Nothing gets there sooner.
    Fastest,
    /// Nothing gets there with fewer changes, and it is not the quickest.
    FewestChanges,
    /// It goes a different way, over this line.
    Via(LineId),
}

impl Merit {
    /// What to put on the card. Needs the map, because a line's name is the
    /// map's to say.
    pub fn label(&self, net: &Network) -> String {
        match self {
            Self::Fastest => "Fastest".to_string(),
            Self::FewestChanges => "Fewest changes".to_string(),
            Self::Via(line) => format!("via {}", net.line(*line).name),
        }
    }
}

/// One of the journeys on offer, and why it is on offer.
#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    pub journey: Journey,
    pub merit: Merit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteError {
    SameStation,
    /// No chain of routable lines joins the two.
    Unreachable,
    /// Every service category has been switched off, so there is nothing to
    /// ride. Said apart from [`Self::Unreachable`] because the map is fine and
    /// the answer is one switch away.
    NothingAllowed,
}

impl fmt::Display for RouteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SameStation => write!(f, "pick two different stations"),
            Self::Unreachable => write!(f, "no routable line connects these two stations"),
            Self::NothingAllowed => {
                write!(f, "every service category is switched off, so there is nothing to ride")
            }
        }
    }
}

impl std::error::Error for RouteError {}

/// The quickest journey between two stations, over the lines the planner is
/// allowed to ride.
pub fn fastest(timing: &Timing, from: StationId, to: StationId) -> Result<Journey, RouteError> {
    if from == to {
        return Err(RouteError::SameStation);
    }
    if !timing.anything_rides() {
        return Err(RouteError::NothingAllowed);
    }
    search(timing, from, to, Aim::Time, &[]).ok_or(RouteError::Unreachable)
}

/// Several ways to make the same journey, quickest first.
///
/// One answer is rarely the whole answer. The quickest way may change twice
/// where another changes once; it may lean on a line somebody would rather
/// not take. So the same machinery is asked a few different questions and the
/// answers that turn out to be genuinely different journeys are offered side
/// by side, each labelled with what makes it worth offering:
///
/// * the quickest,
/// * the one with the fewest changes, when that is not already the quickest,
/// * and the quickest way round each line the first answer leans on, which is
///   how a second corridor through the city gets found.
///
/// Nothing here is a blend of the above with weights invented to make a third
/// card appear. An option is on the list because it is the best answer to a
/// question somebody actually asks, and [`Choice::merit`] says which one.
///
/// Journeys more than [`SPREAD`] times the quickest are dropped: at some point
/// another way to go becomes a curiosity, and a list of curiosities is worse
/// than a short list.
pub fn options(timing: &Timing, from: StationId, to: StationId) -> Result<Vec<Choice>, RouteError> {
    let quickest = fastest(timing, from, to)?;
    let ceiling = quickest.seconds * SPREAD;
    let changes = quickest.transfers();
    let mut kept = vec![Choice { journey: quickest, merit: Merit::Fastest }];

    // Fewest changes is a different question, not a worse answer to the same
    // one — so it is searched for as such, and only offered when it actually
    // saves a change.
    if changes > 0 {
        if let Some(plain) = search(timing, from, to, Aim::Changes, &[]) {
            if plain.transfers() < changes && plain.seconds <= ceiling {
                consider(&mut kept, Choice { journey: plain, merit: Merit::FewestChanges });
            }
        }
    }

    // Another way round: take away one line the quickest journey leans on and
    // ask again. Most-ridden line first, because taking that one away is what
    // forces a genuinely different route rather than a detour round a stop.
    for line in ridden(&kept[0].journey).into_iter().take(OPTIONS + 1) {
        if kept.len() >= OPTIONS {
            break;
        }
        let Some(other) = search(timing, from, to, Aim::Time, &[line]) else {
            continue;
        };
        if other.seconds > ceiling {
            continue;
        }
        // Described by what it rides that the others do not. A journey we
        // cannot tell apart is not worth a card of its own.
        if let Some(apart) = distinctive(&other, &kept) {
            consider(&mut kept, Choice { journey: other, merit: Merit::Via(apart) });
        }
    }

    // Quickest first. A stable sort keeps the fastest at the head, where it
    // already is, and leaves the rest in the order they were found.
    kept.sort_by(|a, b| {
        a.journey.seconds.partial_cmp(&b.journey.seconds).unwrap_or(Ordering::Equal)
    });
    kept.truncate(OPTIONS);
    Ok(kept)
}

/// Keeps a journey unless it is one already on the list wearing another label.
fn consider(kept: &mut Vec<Choice>, fresh: Choice) {
    let same = kept.iter().any(|had| shared(&had.journey, &fresh.journey) > SAME);
    if !same {
        kept.push(fresh);
    }
}

/// How much track two journeys have in common, as a fraction of the shorter.
///
/// Measured in segments rather than kilometres: two routes that differ only
/// over one short stretch are the same route to anyone reading the map, and
/// one that keeps the stretch but skips a station is not.
fn shared(a: &Journey, b: &Journey) -> f64 {
    let edges = |j: &Journey| -> BTreeSet<EdgeId> {
        j.legs.iter().flat_map(|leg| leg.edges.iter().copied()).collect()
    };
    let (one, two) = (edges(a), edges(b));
    let fewest = one.len().min(two.len());
    match fewest {
        0 => 1.0,
        n => one.intersection(&two).count() as f64 / n as f64,
    }
}

/// The lines a journey rides, the most ridden first.
fn ridden(journey: &Journey) -> Vec<LineId> {
    let mut counts: Vec<(LineId, usize)> = Vec::new();
    for leg in &journey.legs {
        match counts.iter_mut().find(|(line, _)| *line == leg.line) {
            Some((_, ridden)) => *ridden += leg.edges.len(),
            None => counts.push((leg.line, leg.edges.len())),
        }
    }
    // Ties broken by line id, so the same map always offers the same options.
    counts.sort_by_key(|(line, ridden)| (Reverse(*ridden), *line));
    counts.into_iter().map(|(line, _)| line).collect()
}

/// The line that makes a journey worth offering beside the ones already on the
/// list: the one it rides most that none of them ride at all.
fn distinctive(journey: &Journey, kept: &[Choice]) -> Option<LineId> {
    let known: Vec<LineId> =
        kept.iter().flat_map(|c| c.journey.legs.iter().map(|leg| leg.line)).collect();
    ridden(journey).into_iter().find(|line| !known.contains(line))
}

/// What a search is minimising.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Aim {
    /// Seconds, and nothing else.
    Time,
    /// Changes first, seconds to settle the ties.
    Changes,
}

/// A running cost. Both of the things anyone minimises are carried the whole
/// way, so that one search can be asked for either and the journey it hands
/// back still knows how long it takes.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Tally {
    changes: usize,
    seconds: f64,
}

impl Tally {
    /// What the search compares, which is the only place the aim shows up.
    ///
    /// Both parts only ever grow along a journey, and the pair is ordered
    /// lexicographically, so Dijkstra is as correct about "fewest changes,
    /// then quickest" as it is about plain seconds.
    fn key(self, aim: Aim) -> (usize, Order) {
        match aim {
            Aim::Time => (0, Order(self.seconds)),
            Aim::Changes => (self.changes, Order(self.seconds)),
        }
    }
}

/// The journey a search settles on, or `None` if it cannot get there.
///
/// Runs over (station, line aboard) rather than plain stations, so that staying
/// put on a line can be preferred to changing at the same spot — which is the
/// whole reason a change can cost more than the ride it saves.
///
/// `blocked` takes lines out of the graph for this search alone. That is how an
/// alternative is found: block what the best answer leans on and see what the
/// city does instead. It is not the same as a switched-off category, which is
/// the user's standing instruction and lives in [`Timing::rides`].
fn search(
    timing: &Timing,
    from: StationId,
    to: StationId,
    aim: Aim,
    blocked: &[LineId],
) -> Option<Journey> {
    let net = timing.net();
    let open = |line: LineId| timing.rides(line) && !blocked.contains(&line);

    type State = (StationId, LineId);
    let mut best: BTreeMap<State, Tally> = BTreeMap::new();
    let mut came: BTreeMap<State, (StationId, Option<LineId>, EdgeId)> = BTreeMap::new();
    let mut queue: BinaryHeap<Reverse<((usize, Order), State)>> = BinaryHeap::new();

    for (next, edge) in net.neighbours(from) {
        if !open(edge.line) {
            continue;
        }
        let state = (next, edge.line);
        let tally = Tally { changes: 0, seconds: timing.step(edge.id, None) };
        if best.get(&state).is_none_or(|had| tally.key(aim) < had.key(aim)) {
            best.insert(state, tally);
            came.insert(state, (from, None, edge.id));
            queue.push(Reverse((tally.key(aim), state)));
        }
    }

    let mut arrival: Option<(State, Tally)> = None;
    while let Some(Reverse((key, state))) = queue.pop() {
        let Some(&here) = best.get(&state) else {
            continue;
        };
        if key > here.key(aim) {
            continue; // a better way here was already settled
        }
        if state.0 == to {
            arrival = Some((state, here));
            break;
        }
        let (at, aboard) = state;
        for (next, edge) in net.neighbours(at) {
            if !open(edge.line) {
                continue;
            }
            let onward = (next, edge.line);
            let total = Tally {
                changes: here.changes + usize::from(edge.line != aboard),
                seconds: here.seconds + timing.step(edge.id, Some(aboard)),
            };
            if best.get(&onward).is_none_or(|had| total.key(aim) < had.key(aim)) {
                best.insert(onward, total);
                came.insert(onward, (at, Some(aboard), edge.id));
                queue.push(Reverse((total.key(aim), onward)));
            }
        }
    }

    let (mut state, tally) = arrival?;

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

    // The seconds reported are the seconds searched on, whichever aim was
    // asked for: the tally carries them either way.
    Some(Journey { legs, seconds: tally.seconds })
}

/// f64 ordered for the heap. Costs here are always finite.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Order(f64);
impl Eq for Order {}
impl Ord for Order {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.partial_cmp(&other.0).unwrap_or(Ordering::Equal)
    }
}
impl PartialOrd for Order {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;
    use crate::timing::Timing;

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

    /// Two little maps in one file, each posing a choice the single answer
    /// cannot express.
    ///
    /// ```text
    ///   Pier --Red-- Mid --Green-- Union        a change, or the long way
    ///     \                         /
    ///      \-------Blue--Bay-------/
    ///
    ///   Dock --Amber-- Mast --Amber-- Wharf     two ways round, one seat each
    ///     \                            /
    ///      \--------Teal--Cove--------/
    /// ```
    const CHOICES: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0,0,24,16">
      <g>
        <path d="M0 0l3 0l0 3l-3 0z" fill="white" stroke="rgb(223,223,247)" stroke-width="0.05"/>
        <text>Key to lines</text>
        <path d="M0.5 0.5l1 0" fill="none" stroke="hsl(0,100%,50%)" stroke-width="0.2"/>
        <text>Red</text>
        <path d="M0.5 1l1 0" fill="none" stroke="hsl(120,100%,30%)" stroke-width="0.2"/>
        <text>Green</text>
        <path d="M0.5 1.5l1 0" fill="none" stroke="hsl(210,100%,50%)" stroke-width="0.2"/>
        <text>Blue</text>
        <path d="M0.5 2l1 0" fill="none" stroke="hsl(40,100%,50%)" stroke-width="0.2"/>
        <text>Amber</text>
        <path d="M0.5 2.5l1 0" fill="none" stroke="hsl(170,100%,40%)" stroke-width="0.2"/>
        <text>Teal</text>
      </g>
      <g>
        <path d="M1.5 1.5L1.5 2.5L1.5 3.5" fill="none" stroke="hsl(0,100%,50%)" stroke-width="0.2"/>
        <path d="M1.5 3.5L1.5 4.5L1.5 5.5" fill="none" stroke="hsl(120,100%,30%)" stroke-width="0.2"/>
        <path d="M1.5 1.5L4.5 2.5L6.5 3.5" fill="none" stroke="hsl(210,100%,50%)" stroke-width="0.2"/>
        <path d="M6.5 3.5L4.5 4.5L1.5 5.5" fill="none" stroke="hsl(210,100%,50%)" stroke-width="0.2"/>
        <path d="M1.5 8.5L1.5 9.5L1.5 10.5" fill="none" stroke="hsl(40,100%,50%)" stroke-width="0.2"/>
        <path d="M1.5 10.5L1.5 11.5L1.5 12.5" fill="none" stroke="hsl(40,100%,50%)" stroke-width="0.2"/>
        <path d="M1.5 8.5L2.5 9.5L3.5 10.5" fill="none" stroke="hsl(170,100%,40%)" stroke-width="0.2"/>
        <path d="M3.5 10.5L2.5 11.5L1.5 12.5" fill="none" stroke="hsl(170,100%,40%)" stroke-width="0.2"/>
      </g>
      <g>
        <ellipse cx="1.5" cy="1.5" rx="0.35" ry="0.35" fill="white" stroke="black" stroke-width="0.16"/>
        <g><text>Pier</text></g>
        <ellipse cx="1.5" cy="3.5" rx="0.35" ry="0.35" fill="white" stroke="black" stroke-width="0.16"/>
        <g><text>Mid</text></g>
        <ellipse cx="1.5" cy="5.5" rx="0.35" ry="0.35" fill="white" stroke="black" stroke-width="0.16"/>
        <g><text>Union</text></g>
        <path d="M6.5 3.275l0 0.45" stroke="hsl(210,100%,50%)" stroke-width="0.2"/>
        <g><text>Bay</text></g>
        <ellipse cx="1.5" cy="8.5" rx="0.35" ry="0.35" fill="white" stroke="black" stroke-width="0.16"/>
        <g><text>Dock</text></g>
        <path d="M1.5 10.275l0 0.45" stroke="hsl(40,100%,50%)" stroke-width="0.2"/>
        <g><text>Mast</text></g>
        <ellipse cx="1.5" cy="12.5" rx="0.35" ry="0.35" fill="white" stroke="black" stroke-width="0.16"/>
        <g><text>Wharf</text></g>
        <path d="M3.5 10.275l0 0.45" stroke="hsl(170,100%,40%)" stroke-width="0.2"/>
        <g><text>Cove</text></g>
      </g>
    </svg>"##;

    fn net() -> Network {
        parse(MAP).expect("fixture parses")
    }

    fn choices() -> Network {
        parse(CHOICES).expect("fixture parses")
    }

    /// A fixture timed a kilometre to the map unit, so that what a journey
    /// costs follows the drawing and nothing else. Two control points make a
    /// straight profile, which is the one case the warp leaves alone.
    fn flat(net: &Network) -> Timing<'_> {
        let straight = crate::Scale::from_points((1.5, 5.5), vec![(40.0, 40.0)]);
        Timing::new(net, &straight, &crate::Services::for_network(net))
    }

    /// The fixture timed as a fresh map is: default scale, categories seeded
    /// from the drawing.
    fn timed(net: &Network) -> Timing<'_> {
        Timing::fresh(net)
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

    /// Every option, as "label: Line n, transfer at X, …", which is the shape
    /// the panel shows.
    fn offered(net: &Network, picks: &[Choice]) -> Vec<String> {
        picks
            .iter()
            .map(|pick| {
                format!("{}: {}", pick.merit.label(net), described(net, &pick.journey).join(", "))
            })
            .collect()
    }

    #[test]
    fn the_quickest_way_and_the_way_with_fewest_changes_are_both_offered() {
        let net = choices();
        let timing = flat(&net);
        let picks = options(&timing, at(&net, "Pier"), at(&net, "Union")).expect("a route");

        // Pier to Union is a change at Mid, or Blue the long way round for a
        // seat the whole way. Both are real answers; which one is better is
        // not the planner's to decide.
        assert_eq!(
            offered(&net, &picks),
            [
                "Fastest: Red 1, transfer at Mid, Green 1",
                "Fewest changes: Blue 2",
            ],
        );
        assert_eq!(picks[0].merit, Merit::Fastest);
        assert_eq!(picks[1].merit, Merit::FewestChanges);
        assert!(picks[0].journey.seconds < picks[1].journey.seconds, "quickest first");
        assert!(picks[1].journey.transfers() < picks[0].journey.transfers(), "and it saves one");
        assert!(
            picks[1].journey.seconds <= picks[0].journey.seconds * SPREAD,
            "{} s against {} s is too far behind to offer",
            picks[1].journey.seconds,
            picks[0].journey.seconds,
        );
    }

    #[test]
    fn another_way_round_is_named_by_the_line_that_makes_it_different() {
        let net = choices();
        let timing = flat(&net);
        let picks = options(&timing, at(&net, "Dock"), at(&net, "Wharf")).expect("a route");

        // Two one-seat rides between the same pair. Neither saves a change, so
        // the only thing to say about the second is where it goes.
        assert_eq!(
            offered(&net, &picks),
            ["Fastest: Amber 2", "via Teal: Teal 2"],
        );
        let teal = net.lines.iter().find(|l| l.name == "Teal").expect("the Teal line").id;
        assert_eq!(picks[1].merit, Merit::Via(teal));
        // And it is a different way round, not the same track relabelled.
        let ridden = |pick: &Choice| -> Vec<EdgeId> {
            pick.journey.legs.iter().flat_map(|leg| leg.edges.iter().copied()).collect()
        };
        for edge in ridden(&picks[1]) {
            assert!(!ridden(&picks[0]).contains(&edge), "both options ride {edge:?}");
        }
    }

    #[test]
    fn every_option_is_timed_by_the_model_that_chose_it() {
        // The invariant that matters for one journey matters for all of them:
        // a card showing a time that was not what its route was picked by is
        // worse than no card.
        let net = choices();
        let timing = flat(&net);
        for (a, b) in [("Pier", "Union"), ("Dock", "Wharf"), ("Mid", "Bay")] {
            for pick in options(&timing, at(&net, a), at(&net, b)).expect("a route") {
                assert!(
                    (pick.journey.seconds - timing.journey(&pick.journey)).abs() < 1e-9,
                    "{a} to {b}, {}: searched {} s, reported {} s",
                    pick.merit.label(&net),
                    pick.journey.seconds,
                    timing.journey(&pick.journey),
                );
            }
        }
    }

    #[test]
    fn a_detour_nobody_would_take_is_not_an_option() {
        let net = net();
        let timing = timed(&net);
        // Scenic joins Pier to Union with no change at all, and takes five
        // times as long. A list is only worth reading if everything on it is
        // worth taking.
        let picks = options(&timing, at(&net, "Pier"), at(&net, "Union")).expect("a route");
        assert_eq!(offered(&net, &picks), ["Fastest: North 1, transfer at Quay, East 2"]);

        // Same for a hop with only one way to make it.
        let one = options(&timing, at(&net, "Pier"), at(&net, "Quay")).expect("a route");
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].merit, Merit::Fastest);
    }

    #[test]
    fn the_options_are_at_most_a_handful_and_never_the_same_journey_twice() {
        let net = choices();
        let timing = flat(&net);
        for (a, b) in [("Pier", "Union"), ("Dock", "Wharf"), ("Pier", "Bay")] {
            let picks = options(&timing, at(&net, a), at(&net, b)).expect("a route");
            assert!(!picks.is_empty() && picks.len() <= OPTIONS, "{} for {a} to {b}", picks.len());
            for pair in picks.windows(2) {
                assert!(pair[0].journey.seconds <= pair[1].journey.seconds, "in time order");
                assert_ne!(pair[0].journey.legs, pair[1].journey.legs, "the same journey twice");
            }
        }
    }

    #[test]
    fn asking_for_options_is_refused_for_the_same_reasons_as_asking_for_one() {
        let net = net();
        let timing = timed(&net);
        let pier = at(&net, "Pier");
        assert_eq!(options(&timing, pier, pier), Err(RouteError::SameStation));
        assert_eq!(options(&timing, pier, at(&net, "Islet")), Err(RouteError::Unreachable));

        let mut services = crate::Services::for_network(&net);
        services.set_allowed(0, false);
        let shut = Timing::new(&net, &crate::Scale::for_network(&net), &services);
        assert_eq!(
            options(&shut, pier, at(&net, "Union")),
            Err(RouteError::NothingAllowed),
        );
    }

    #[test]
    fn the_quickest_option_is_the_one_a_single_answer_gives() {
        // Two entry points, one planner: whatever `fastest` says has to be the
        // head of the list, or the panel and the map would disagree.
        let net = choices();
        let timing = flat(&net);
        for (a, b) in [("Pier", "Union"), ("Dock", "Wharf"), ("Bay", "Mid")] {
            let (from, to) = (at(&net, a), at(&net, b));
            let one = fastest(&timing, from, to).expect("a route");
            let picks = options(&timing, from, to).expect("a route");
            assert_eq!(picks[0].journey, one, "{a} to {b}");
        }
    }

    #[test]
    fn a_single_line_journey_has_one_leg_and_no_transfer() {
        let net = net();
        let j = fastest(&timed(&net), at(&net, "Pier"), at(&net, "Ridge")).unwrap();
        assert_eq!(described(&net, &j), ["North 2"]);
        assert_eq!(j.transfers(), 0);
        assert_eq!(j.stations(), 2);
        assert!(j.transfer_points().is_empty());
    }

    #[test]
    fn changing_lines_is_reported_at_the_right_station() {
        let net = net();
        let j = fastest(&timed(&net), at(&net, "Pier"), at(&net, "Union")).unwrap();
        assert_eq!(described(&net, &j), ["North 1", "transfer at Quay", "East 2"]);
        assert_eq!(j.transfers(), 1);
        assert_eq!(j.stations(), 3);
        assert_eq!(j.transfer_points(), vec![at(&net, "Quay")]);
    }

    #[test]
    fn a_change_beats_a_much_longer_one_seat_ride() {
        let net = net();
        let j = fastest(&timed(&net), at(&net, "Pier"), at(&net, "Union")).unwrap();
        // Scenic runs Pier -> Cape -> Union with no change at all, but ~20 units
        assert!(
            j.legs.iter().all(|l| net.line(l.line).name != "Scenic"),
            "took the scenic route: {:?}", described(&net, &j),
        );
        assert!(j.seconds > 0.0 && j.seconds < 3_600.0, "{} s is not a sane time", j.seconds);
    }

    #[test]
    fn one_seat_wins_when_the_distance_is_comparable() {
        let net = net();
        // Pier -> Cape is Scenic all the way; going North then East then Scenic
        // back out would be absurd, and the transfer charge guarantees it loses
        let j = fastest(&timed(&net), at(&net, "Pier"), at(&net, "Cape")).unwrap();
        assert_eq!(described(&net, &j), ["Scenic 1"]);
    }

    #[test]
    fn journeys_are_symmetric() {
        let net = net();
        let there = fastest(&timed(&net), at(&net, "Pier"), at(&net, "Union")).unwrap();
        let back = fastest(&timed(&net), at(&net, "Union"), at(&net, "Pier")).unwrap();
        assert!(
            (there.seconds - back.seconds).abs() < 1e-9,
            "{} s out, {} s back",
            there.seconds,
            back.seconds,
        );
        assert_eq!(there.transfers(), back.transfers());
    }

    #[test]
    fn legs_join_end_to_end() {
        let net = net();
        let j = fastest(&timed(&net), at(&net, "Ridge"), at(&net, "Union")).unwrap();
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
        let j = fastest(&timed(&net), at(&net, "Pier"), at(&net, "Union")).unwrap();
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

    /// The fixture with Scenic — the long way round — in a category of its
    /// own, which is the only way to switch one kind of service off and not
    /// the other.
    fn split(net: &Network) -> crate::Services {
        let mut services = crate::Services::for_network(net);
        let apart = services.add("Scenic");
        let scenic = net.lines.iter().find(|l| l.name == "Scenic").expect("the Scenic line").id;
        services.assign(scenic, apart);
        services
    }

    fn timed_with<'a>(net: &'a Network, services: &crate::Services) -> Timing<'a> {
        Timing::new(net, &crate::Scale::for_network(net), services)
    }

    #[test]
    fn a_switched_off_category_is_not_ridden() {
        let net = net();
        let mut services = split(&net);
        // Pier to Union is a change at Quay while everything is on — Scenic
        // goes the long way round and loses, as its own test above says.
        let both = timed_with(&net, &services);
        assert_eq!(described(&net, &fastest(&both, at(&net, "Pier"), at(&net, "Union")).unwrap()),
                   ["North 1", "transfer at Quay", "East 2"]);

        // Switch the metro off and the long way round is all that is left: a
        // worse journey, which is the honest answer to the question asked.
        services.set_allowed(0, false);
        let scenic_only = timed_with(&net, &services);
        let detour = fastest(&scenic_only, at(&net, "Pier"), at(&net, "Union")).unwrap();
        assert_eq!(described(&net, &detour), ["Scenic 2"]);
        assert!(
            detour.seconds > fastest(&both, at(&net, "Pier"), at(&net, "Union")).unwrap().seconds,
            "the way round cannot be the quicker one",
        );
    }

    #[test]
    fn switching_a_category_off_can_cut_a_station_off_with_it() {
        let net = net();
        let mut services = split(&net);
        // Cape is on Scenic and nothing else.
        let cape = at(&net, "Cape");
        assert!(fastest(&timed_with(&net, &services), at(&net, "Pier"), cape).is_ok());
        services.set_allowed(1, false);
        assert_eq!(
            fastest(&timed_with(&net, &services), at(&net, "Pier"), cape),
            Err(RouteError::Unreachable),
            "the map still joins them; nothing we may ride does",
        );
        // And what is left is routed as if the line were not drawn at all.
        let rest = fastest(&timed_with(&net, &services), at(&net, "Pier"), at(&net, "Union"));
        assert_eq!(described(&net, &rest.unwrap()), ["North 1", "transfer at Quay", "East 2"]);
    }

    #[test]
    fn with_every_category_off_it_says_so_rather_than_blaming_the_map() {
        let net = net();
        let mut services = split(&net);
        for at in 0..services.categories().len() {
            services.set_allowed(at, false);
        }
        let shut = timed_with(&net, &services);
        assert_eq!(
            fastest(&shut, at(&net, "Pier"), at(&net, "Union")),
            Err(RouteError::NothingAllowed),
        );
        // Still the first thing checked: two of the same station is a mistake
        // whatever is switched on.
        let pier = at(&net, "Pier");
        assert_eq!(fastest(&shut, pier, pier), Err(RouteError::SameStation));
    }

    #[test]
    fn says_so_when_there_is_no_route() {
        let net = net();
        let pier = at(&net, "Pier");
        assert_eq!(fastest(&timed(&net), pier, pier), Err(RouteError::SameStation));
        // Orphan is a perfectly good line, just not joined to anything
        assert_eq!(fastest(&timed(&net), pier, at(&net, "Islet")), Err(RouteError::Unreachable));
        assert!(fastest(&timed(&net), at(&net, "Islet"), at(&net, "Jetty")).is_ok());
    }
}
