//! How long a journey takes.
//!
//! Everything needed for that now exists separately: [`Scale`] turns drawn
//! track into kilometres, and a line's [`Services`] category says how fast it
//! covers them and how hard it speeds up and slows down. This module is where
//! the two meet, and it is the only place that decides what a journey costs —
//! the planner searches on exactly the seconds reported afterwards, so the
//! route shown and the time shown can never come from different models.
//!
//! A change between lines costs two different things, and they are kept
//! apart because only one of them is about the stations:
//!
//! * The walk across to the other platform ([`Services::walk`]), which is one
//!   figure for the whole map, since nothing in the file says how big a
//!   station is.
//! * The wait, which is **half the headway** of the line being boarded —
//!   the average for a passenger who turns up without consulting a timetable,
//!   which is how people use a metro and roughly how they use anything that
//!   comes often. It is the frequency, not the station, that makes a change at
//!   a twenty-minute commuter line cost four times one at a five-minute metro.
//!
//! [`DWELL`], the stand at an intermediate stop, is the remaining flat number.
//!
//! Boarding the *first* service is not charged a wait. The journey time is
//! time spent travelling, and a passenger starting out can look up a departure
//! in a way that one already mid-journey cannot. Comparing two routes is
//! unaffected either way whenever they start on the same line, and when they
//! do not, it is the kind of thing a departure board answers and this model
//! cannot.
//!
//! The per-edge times are worked out once, when a `Timing` is built, because
//! a scale is dragged live and a search that re-measured every segment through
//! the warp on every relaxation would do it thousands of times a frame.

use crate::network::{EdgeId, LineId, Network};
use crate::route::{Journey, Leg};
use crate::scale::Scale;
use crate::service::Services;

/// Seconds stood at a stop along the way.
pub const DWELL: f64 = 30.0;


/// What riding this map costs, in seconds.
///
/// Built from a network, a scale and a set of services, and worth rebuilding
/// whenever any of the three changes — which is cheap, and is why it borrows
/// the map rather than holding a copy of it.
#[derive(Debug, Clone)]
pub struct Timing<'a> {
    net: &'a Network,
    /// Seconds to ride each edge, stop to stop, by edge index.
    seconds: Vec<f64>,
    /// Seconds of expected wait for each line, by line index: half its
    /// headway.
    waits: Vec<f64>,
    /// Seconds stood at an intermediate stop.
    pub dwell: f64,
    /// Seconds to walk across an interchange.
    pub walk: f64,
}

impl<'a> Timing<'a> {
    /// Times every segment of the map under a scale and a set of services.
    pub fn new(net: &'a Network, scale: &Scale, services: &Services) -> Self {
        let seconds = net
            .edges
            .iter()
            .map(|edge| {
                let km = scale.length(&edge.polyline);
                let at = services.category_of(edge.line);
                match services.categories().get(at) {
                    Some(category) => category.run_time(km),
                    None => 0.0,
                }
            })
            .collect();
        let waits = net
            .lines
            .iter()
            .map(|line| services.headway_of(line.id) * 60.0 / 2.0)
            .collect();
        Self { net, seconds, waits, dwell: DWELL, walk: services.walk() }
    }

    /// The map timed with a fresh scale and fresh service categories, which is
    /// what a map is worth before anyone has said otherwise.
    pub fn fresh(net: &'a Network) -> Self {
        Self::new(net, &Scale::for_network(net), &Services::for_network(net))
    }

    pub fn net(&self) -> &'a Network {
        self.net
    }

    /// Seconds to ride one segment, stop to stop.
    pub fn edge(&self, id: EdgeId) -> f64 {
        self.seconds.get(id.0).copied().unwrap_or(0.0)
    }

    /// Seconds of expected wait for a line: half the gap between its services.
    pub fn wait(&self, line: LineId) -> f64 {
        self.waits.get(line.0).copied().unwrap_or(0.0)
    }

    /// Seconds to change onto a line: the walk, and then the wait.
    pub fn boarding(&self, line: LineId) -> f64 {
        self.walk + self.wait(line)
    }

    /// Seconds aboard for one leg: every segment, and a stand at each stop
    /// between them. Boarding and alighting are the journey's business, not
    /// the leg's.
    pub fn leg(&self, leg: &Leg) -> f64 {
        let riding: f64 = leg.edges.iter().map(|id| self.edge(*id)).sum();
        riding + self.dwell * leg.edges.len().saturating_sub(1) as f64
    }

    /// Seconds from boarding the first service to stepping off the last.
    pub fn journey(&self, journey: &Journey) -> f64 {
        let riding: f64 = journey.legs.iter().map(|leg| self.leg(leg)).sum();
        // Every leg but the first was changed onto, and what that cost depends
        // on how often the line it changed onto runs.
        let changes: f64 = journey.legs.iter().skip(1).map(|leg| self.boarding(leg.line)).sum();
        riding + changes
    }

    /// What riding `edge` adds, having arrived on `arrived_on` — nothing extra
    /// when boarding at the origin, a stand when staying aboard, the change
    /// when swapping lines. The planner's cost function, and the reason the
    /// reported time matches the one searched on.
    pub(crate) fn step(&self, edge: EdgeId, arrived_on: Option<LineId>) -> f64 {
        let riding = self.edge(edge);
        let onto = self.net.edge(edge).line;
        match arrived_on {
            None => riding,
            Some(line) if line == onto => riding + self.dwell,
            Some(_) => riding + self.boarding(onto),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::MAX_HEADWAY;
    use crate::{fastest, parse, Mode, Services, StationId};

    /// Two lines over five stations, one of them slow.
    const MAP: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0,0,24,12">
      <g>
        <path d="M0 0l3 0l0 3l-3 0z" fill="white" stroke="rgb(223,223,247)" stroke-width="0.05"/>
        <text>Key to lines</text>
        <path d="M0.5 0.5l1 0" fill="none" stroke="hsl(0,100%,50%)" stroke-width="0.2"/>
        <text>North</text>
        <path d="M0.5 1l1 0" fill="none" stroke="hsl(210,100%,50%)" stroke-width="0.2"/>
        <text>East</text>
      </g>
      <g>
        <path d="M1.5 1.5L1.5 2.5L1.5 3.5" fill="none" stroke="hsl(0,100%,50%)" stroke-width="0.2"/>
        <path d="M1.5 3.5L1.5 4.5L1.5 5.5" fill="none" stroke="hsl(0,100%,50%)" stroke-width="0.2"/>
        <path d="M1.5 3.5L2.5 3.5L3.5 3.5" fill="none" stroke="hsl(210,100%,50%)" stroke-width="0.2"/>
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
      </g>
    </svg>"##;

    fn net() -> Network {
        parse(MAP).expect("fixture parses")
    }

    fn at(net: &Network, name: &str) -> StationId {
        net.station_named(name).unwrap_or_else(|| panic!("no station {name}"))
    }

    #[test]
    fn a_journey_costs_what_the_search_was_told_it_costs() {
        // The one invariant that matters: if these drift, the app shows a time
        // for a route that was chosen by a different number.
        let net = net();
        let timing = Timing::fresh(&net);
        for (a, b) in [("Pier", "Ridge"), ("Pier", "Tide"), ("Ridge", "Tide")] {
            let journey = fastest(&timing, at(&net, a), at(&net, b)).expect("a route");
            assert!(
                (journey.seconds - timing.journey(&journey)).abs() < 1e-9,
                "{a} to {b}: searched {} s, reported {} s",
                journey.seconds,
                timing.journey(&journey),
            );
        }
    }

    #[test]
    fn the_parts_of_a_journey_add_up_to_the_whole() {
        let net = net();
        let timing = Timing::fresh(&net);
        let journey = fastest(&timing, at(&net, "Pier"), at(&net, "Tide")).expect("a route");
        assert_eq!(journey.transfers(), 1, "Pier to Tide is a change at Quay");

        let legs: f64 = journey.legs.iter().map(|leg| timing.leg(leg)).sum();
        let change = timing.boarding(journey.legs[1].line);
        assert!((journey.seconds - legs - change).abs() < 1e-9);

        // And a leg is its segments plus a stand at each stop in between.
        for leg in &journey.legs {
            let riding: f64 = leg.edges.iter().map(|id| timing.edge(*id)).sum();
            let stands = timing.dwell * (leg.edges.len() - 1) as f64;
            assert!((timing.leg(leg) - riding - stands).abs() < 1e-9);
        }
    }

    #[test]
    fn a_slower_category_makes_for_a_longer_ride() {
        let net = net();
        let quick = Timing::fresh(&net);
        let before = quick.edge(crate::EdgeId(0));

        let mut services = Services::for_network(&net);
        services.set_top_speed(0, 20.0);
        services.set_accel(0, 0.3);
        let crawling = Timing::new(&net, &Scale::for_network(&net), &services);
        assert!(
            crawling.edge(crate::EdgeId(0)) > before * 1.5,
            "{} s should be well over {before} s",
            crawling.edge(crate::EdgeId(0)),
        );
    }

    #[test]
    fn a_stretched_scale_makes_for_a_longer_ride_too() {
        let net = net();
        let plain = Timing::fresh(&net);
        let reach = Scale::for_network(&net).reach();
        let far = Scale::from_points(Scale::for_network(&net).centre, vec![(reach, reach * 8.0)]);
        let stretched = Timing::new(&net, &far, &Services::for_network(&net));
        assert!(stretched.edge(crate::EdgeId(0)) > plain.edge(crate::EdgeId(0)));
    }

    #[test]
    fn a_change_costs_the_walk_plus_half_the_wait_for_what_is_boarded() {
        let net = net();
        let mut services = Services::for_network(&net);
        let east = net.lines.iter().find(|l| l.name == "East").expect("the East line").id;

        let often = Timing::new(&net, &Scale::for_network(&net), &services);
        assert!(
            (often.boarding(east) - often.walk - often.wait(east)).abs() < 1e-9,
            "a change is the walk and then the wait",
        );
        assert!((often.wait(east) - 5.0 * 60.0 / 2.0).abs() < 1e-9, "half of a five minute gap");

        // The walk is the user's to set, and is added to every wait.
        services.set_walk(240.0);
        let strolling = Timing::new(&net, &Scale::for_network(&net), &services);
        assert!(
            (strolling.boarding(east) - often.boarding(east) - 180.0).abs() < 1e-9,
            "three more minutes of walking is three more minutes",
        );
        services.set_walk(crate::service::DEFAULT_WALK);

        // A line that runs every half hour is a far worse thing to change onto
        // than one that runs every five minutes, and only the frequency says so.
        services.set_own_headway(east, Some(30.0));
        let rarely = Timing::new(&net, &Scale::for_network(&net), &services);
        assert!((rarely.wait(east) - 15.0 * 60.0).abs() < 1e-9);
        assert!(
            rarely.boarding(east) > often.boarding(east) + 12.0 * 60.0,
            "{} s against {} s",
            rarely.boarding(east),
            often.boarding(east),
        );
        // The walk is the only part the frequency does not move.
        assert_eq!(rarely.walk, often.walk);
    }

    #[test]
    fn a_rare_line_is_worth_avoiding_even_when_it_is_quicker_to_ride() {
        let net = net();
        let mut services = Services::for_network(&net);
        let east = net.lines.iter().find(|l| l.name == "East").expect("the East line").id;
        services.set_own_headway(east, Some(MAX_HEADWAY));
        let timing = Timing::new(&net, &Scale::for_network(&net), &services);

        let journey = fastest(&timing, at(&net, "Pier"), at(&net, "Tide")).expect("a route");
        // Still the only way there, but the hour it now takes is the wait.
        assert_eq!(journey.transfers(), 1);
        assert!(
            journey.seconds > MAX_HEADWAY * 60.0 / 2.0,
            "{} s should be dominated by the wait",
            journey.seconds,
        );
    }

    #[test]
    fn an_edge_that_does_not_exist_costs_nothing_rather_than_panicking() {
        let net = net();
        let timing = Timing::fresh(&net);
        assert_eq!(timing.edge(crate::EdgeId(9_999)), 0.0);
        assert_eq!(timing.net().lines[0].mode, Mode::Metro);
    }
}
