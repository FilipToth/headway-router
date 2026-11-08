//! Real-world distance, which a Tennessine map does not carry.
//!
//! The file has no scale bar and nothing in kilometres (format notes, gotcha 5),
//! and worse, a schematic map is not drawn to one scale at all: the centre is
//! stretched so the interchanges and their labels fit, the suburbs squeezed so
//! that the map ends somewhere. What stays roughly true of a map drawn that way
//! is that the distortion is **radial** — what a map unit is worth depends
//! mostly on how far out it is.
//!
//! So the user supplies two things, and this module turns them into distances:
//! where the centre of the city is, and a profile giving the real distance from
//! that centre of a point drawn `r` units from it. The profile runs **smoothly**
//! through its control points rather than straight between them — see
//! [`Scale::real_radius`], because that choice is felt, not cosmetic. Together they are a radial
//! warp of the plane, and the real length of a piece of track is the length of
//! its warped polyline — which is why [`Scale::length`] samples along a segment
//! instead of trusting its endpoints (see [`STEP`]).
//!
//! A fresh profile is the shape of the distortion rather than a measurement of
//! it: drawn large in the middle, squeezed at the edges (see [`CENTRE_RATE`]).
//! Its overall size is arbitrary — nothing in the file could tell us — so it is
//! pinned to the map's own extent and meant to be dragged.

use crate::network::Network;
use crate::route::{Journey, Leg};
use crate::Cell;
use std::cmp::Ordering;

/// Longest piece of a drawn segment measured in one straight step, in map
/// units. The warp bends a straight segment — a line passing near the centre
/// bends most — so measuring endpoint to endpoint would return the chord of
/// that bend. A quarter of a cell is far finer than any curvature the profile
/// can produce at the distances stations actually sit at.
const STEP: f64 = 0.25;

/// Control points in a fresh profile, counting the pinned origin.
const POINTS: usize = 6;

/// Closest two control points may sit, in map units. A profile is allowed to be
/// a steep ramp; it is not allowed to be a vertical cliff, which is a division
/// by nearly zero when the slope is read.
pub const MIN_GAP: f64 = 0.5;

/// Smallest profile a map can get, for a map whose stations all share one cell.
const MIN_REACH: f64 = 5.0;

/// The shape of a fresh profile: `km = CENTRE_RATE·x + BULGE·x³`, in fractions
/// of the map's reach.
///
/// A schematic map is not uniform and a straight profile is the one shape it
/// certainly is not. The middle is drawn large so the interchanges and their
/// labels fit, the suburbs squeezed so the map ends, so a map unit is worth
/// little in the centre and several times that out at the rim. These two
/// numbers put a fresh map at **300 m per unit at the centre rising to 2 km at
/// the edge**, with the furthest station still about `reach` km out — the same
/// overall size the old straight default gave, redistributed the way a drawn
/// map actually distorts.
///
/// Both are guesses at a shape, not at a city. Nothing in a Tennessine file
/// could tell us either, which is what the settings panel is for.
pub const CENTRE_RATE: f64 = 0.272;
/// How much the profile bends away from [`CENTRE_RATE`] towards the edge.
pub const BULGE: f64 = 0.708;

/// How map distance becomes real distance.
///
/// Two settings: [`centre`](Self::centre), the point radii are measured from,
/// and a profile of `(map units from centre, km from centre)` control points
/// that the user drags. Both are edited live, so this is cheap to clone and
/// compares by value.
#[derive(Debug, Clone, PartialEq)]
pub struct Scale {
    /// The centre of the city in map units — dragged on the map itself.
    pub centre: (f64, f64),
    /// How far out the profile is drawn: the furthest station from the centre
    /// the map was loaded with. Past it the final slope simply carries on, so
    /// moving the centre can never put track beyond measuring.
    reach: f64,
    /// `(map units, km)`, ascending in both, the first pinned at the origin
    /// because the centre is no distance from itself. Never fewer than two, so
    /// there is always a slope to extrapolate with.
    points: Vec<(f64, f64)>,
}

impl Scale {
    /// A fresh profile for a map: centred on its stations, shaped by
    /// [`CENTRE_RATE`] and [`BULGE`].
    ///
    /// The centre is snapped to a cell centre so the readout starts tidy, and
    /// the reach rounded out to a multiple of five so the graph's axis does.
    pub fn for_network(net: &Network) -> Self {
        let mut sum = (0.0, 0.0);
        for s in &net.stations {
            let (x, y) = s.cell.centre();
            sum = (sum.0 + x, sum.1 + y);
        }
        let centre = match net.stations.len() {
            0 => {
                let vb = net.view_box;
                (vb.min_x + vb.width / 2.0, vb.min_y + vb.height / 2.0)
            }
            n => Cell::snap(sum.0 / n as f64, sum.1 / n as f64).centre(),
        };

        let furthest = net
            .stations
            .iter()
            .map(|s| {
                let (x, y) = s.cell.centre();
                (x - centre.0).hypot(y - centre.1)
            })
            .fold(0.0, f64::max);
        let reach = ((furthest / MIN_REACH).ceil() * MIN_REACH).max(MIN_REACH);

        let last = POINTS - 1;
        let points = (0..POINTS)
            .map(|i| {
                let out = i as f64 / last as f64;
                (reach * out, reach * (CENTRE_RATE * out + BULGE * out * out * out))
            })
            .collect();
        Self { centre, reach, points }
    }

    /// A profile built straight from control points, for tests and for settings
    /// restored from elsewhere.
    ///
    /// The input is sanitised rather than rejected: points are sorted, ones
    /// that are not finite or not positive are dropped, ones closer together
    /// than [`MIN_GAP`] are dropped, and a kilometre value that would double
    /// back is lifted to its neighbour's. The origin is supplied.
    pub fn from_points(centre: (f64, f64), raw: Vec<(f64, f64)>) -> Self {
        let mut sorted: Vec<(f64, f64)> = raw
            .into_iter()
            .filter(|(r, km)| r.is_finite() && km.is_finite() && *r > 0.0 && *km >= 0.0)
            .collect();
        sorted.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(Ordering::Equal));

        let mut points = vec![(0.0, 0.0)];
        for (r, km) in sorted {
            let last = points[points.len() - 1];
            if r < last.0 + MIN_GAP {
                continue;
            }
            points.push((r, km.max(last.1)));
        }
        if points.len() == 1 {
            points.push((MIN_REACH, MIN_REACH));
        }
        let reach = points[points.len() - 1].0;
        Self { centre, reach, points }
    }

    /// The profile's control points, `(map units from centre, km from centre)`.
    pub fn points(&self) -> &[(f64, f64)] {
        &self.points
    }

    /// The radius the profile is drawn out to, which is the graph's x axis.
    pub fn reach(&self) -> f64 {
        self.reach
    }

    /// Moves control point `i`, returning where it actually ended up.
    ///
    /// The profile has to stay a rising function of radius or it stops meaning
    /// anything, so a point cannot pass either neighbour on either axis, and
    /// point 0 cannot move at all.
    pub fn move_point(&mut self, i: usize, r: f64, km: f64) -> (f64, f64) {
        let here = self.points.get(i).copied().unwrap_or((0.0, 0.0));
        if i == 0 || i >= self.points.len() || !r.is_finite() || !km.is_finite() {
            return here;
        }
        let (prev_r, prev_km) = self.points[i - 1];
        let next = self.points.get(i + 1).copied();
        let low_r = prev_r + MIN_GAP;
        let high_r = next.map_or(self.reach, |(r, _)| r - MIN_GAP).max(low_r);
        let high_km = next.map_or(f64::INFINITY, |(_, km)| km).max(prev_km);

        let moved = (r.clamp(low_r, high_r), km.clamp(prev_km, high_km));
        self.points[i] = moved;
        moved
    }

    /// Real distance from the centre, in km, of a point drawn `r` units out.
    ///
    /// A smooth (monotone cubic) curve through the control points, not straight
    /// lines between them. The difference is the whole behaviour of the model:
    /// what converts a piece of track is the profile's **slope**, and straight
    /// lines make the slope jump at every control point. A user dragging a
    /// point sees a gentle bend and gets a cliff — two neighbouring stretches
    /// of track, one either side of a point, measured at rates several times
    /// apart with nothing on screen to say why. The curve keeps the slope
    /// continuous, so distance changes gradually across the map as a real
    /// city's does, while still passing exactly through every point dragged.
    pub fn real_radius(&self, r: f64) -> f64 {
        // Spelled out rather than `!(r > 0.0)` so that a radius which is not a
        // number is turned away here, where it is obvious, instead of coming
        // back out of the interpolation below as a NaN distance.
        if !r.is_finite() || r <= 0.0 {
            return 0.0;
        }
        let last = self.points.len() - 1;
        for i in 0..last {
            let ((r0, k0), (r1, k1)) = (self.points[i], self.points[i + 1]);
            if r <= r1 {
                let h = r1 - r0;
                if h <= 0.0 {
                    return k1;
                }
                let (t, m0, m1) = ((r - r0) / h, self.tangent(i), self.tangent(i + 1));
                let (t2, t3) = (t * t, t * t * t);
                return (2.0 * t3 - 3.0 * t2 + 1.0) * k0
                    + (t3 - 2.0 * t2 + t) * h * m0
                    + (-2.0 * t3 + 3.0 * t2) * k1
                    + (t3 - t2) * h * m1;
            }
        }
        // Past the last point the curve carries on straight at its final slope.
        let (r1, k1) = self.points[last];
        k1 + (r - r1) * self.tangent(last)
    }

    /// Km per map unit at radius `r`: the profile's slope, which is what
    /// actually converts a piece of track into a distance.
    ///
    /// Differentiated from the same curve [`Self::real_radius`] evaluates, so a
    /// readout of it can never disagree with the distances on the map.
    pub fn km_per_unit(&self, r: f64) -> f64 {
        let last = self.points.len() - 1;
        if !r.is_finite() || r <= 0.0 {
            return self.tangent(0);
        }
        for i in 0..last {
            let ((r0, k0), (r1, k1)) = (self.points[i], self.points[i + 1]);
            if r <= r1 {
                let h = r1 - r0;
                if h <= 0.0 {
                    return 0.0;
                }
                let (t, m0, m1) = ((r - r0) / h, self.tangent(i), self.tangent(i + 1));
                let t2 = t * t;
                return (6.0 * t2 - 6.0 * t) * (k0 - k1) / h
                    + (3.0 * t2 - 4.0 * t + 1.0) * m0
                    + (3.0 * t2 - 2.0 * t) * m1;
            }
        }
        self.tangent(last)
    }

    /// The straight-line slope of the span starting at control point `i`.
    fn secant(&self, i: usize) -> f64 {
        let ((r0, k0), (r1, k1)) = (self.points[i], self.points[i + 1]);
        if r1 > r0 {
            (k1 - k0) / (r1 - r0)
        } else {
            0.0
        }
    }

    /// The curve's slope at control point `i`.
    ///
    /// The Fritsch–Carlson weighted harmonic mean of the spans either side,
    /// which is what keeps the curve monotone: it never exceeds three times
    /// the shallower neighbour, so the cubic cannot overshoot and send a
    /// distance backwards between two points the user put in order.
    fn tangent(&self, i: usize) -> f64 {
        let last = self.points.len() - 1;
        if i == 0 {
            return self.secant(0);
        }
        if i >= last {
            return self.secant(last - 1);
        }
        let (before, after) = (self.secant(i - 1), self.secant(i));
        if before <= 0.0 || after <= 0.0 {
            // A flat span either side: level off, rather than bulge over the
            // point and come back down.
            return 0.0;
        }
        let h0 = self.points[i].0 - self.points[i - 1].0;
        let h1 = self.points[i + 1].0 - self.points[i].0;
        let (w0, w1) = (2.0 * h1 + h0, h1 + 2.0 * h0);
        (w0 + w1) / (w0 / before + w1 / after)
    }

    /// Where a drawn point lands in real space, in km relative to the centre.
    ///
    /// Only distances between warped points mean anything, so the centre is the
    /// origin and no attempt is made to hold the rest of the map still.
    pub fn warp(&self, (x, y): (f64, f64)) -> (f64, f64) {
        let (dx, dy) = (x - self.centre.0, y - self.centre.1);
        let r = dx.hypot(dy);
        if !r.is_finite() || r <= 0.0 {
            return (0.0, 0.0);
        }
        let k = self.real_radius(r) / r;
        (dx * k, dy * k)
    }

    /// The real length of a drawn polyline, in km.
    pub fn length(&self, points: &[(f64, f64)]) -> f64 {
        let mut total = 0.0;
        for pair in points.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            let span = (b.0 - a.0).hypot(b.1 - a.1);
            if !span.is_finite() {
                continue;
            }
            let steps = (span / STEP).ceil().max(1.0);
            let mut prev = self.warp(a);
            for s in 1..=(steps as usize) {
                let t = s as f64 / steps;
                let here = self.warp((a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t));
                total += (here.0 - prev.0).hypot(here.1 - prev.1);
                prev = here;
            }
        }
        total
    }

    /// How far a rider travels on one leg, in km.
    pub fn leg_km(&self, net: &Network, leg: &Leg) -> f64 {
        self.length(&leg.polyline(net))
    }

    /// How far a rider travels in all, in km. Legs meet at a point, so this is
    /// their sum with nothing counted twice.
    pub fn journey_km(&self, net: &Network, journey: &Journey) -> f64 {
        journey.legs.iter().map(|leg| self.leg_km(net, leg)).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::polyline_length;

    fn profile(points: &[(f64, f64)]) -> Scale {
        Scale::from_points((0.0, 0.0), points.to_vec())
    }

    fn apart(a: (f64, f64), b: (f64, f64)) -> f64 {
        (b.0 - a.0).hypot(b.1 - a.1)
    }

    #[test]
    fn a_one_to_one_profile_is_one_km_per_map_unit() {
        let s = profile(&[(10.0, 10.0), (50.0, 50.0)]);
        let track = [(3.0, 4.0), (3.0, 10.0), (9.0, 16.0), (9.0, 40.0)];
        let drawn = polyline_length(&track);
        assert!(
            (s.length(&track) - drawn).abs() < 1e-6,
            "{} km for {drawn} units of track",
            s.length(&track),
        );
        assert!((s.km_per_unit(0.0) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn the_centre_is_the_origin_of_real_space() {
        let s = Scale::from_points((-14.5, -30.5), vec![(20.0, 8.0), (60.0, 60.0)]);
        assert_eq!(s.warp((-14.5, -30.5)), (0.0, 0.0));
        assert_eq!(s.real_radius(0.0), 0.0);
        assert_eq!(s.real_radius(-3.0), 0.0, "a radius is never negative");
        assert_eq!(s.length(&[(-14.5, -30.5)]), 0.0, "a single point goes nowhere");
    }

    #[test]
    fn a_stretched_centre_shortens_the_hops_drawn_there() {
        // Ten map units of city centre are worth 2 km; out in the suburbs a
        // map unit is worth a full kilometre.
        let s = profile(&[(10.0, 2.0), (50.0, 42.0)]);
        let central = s.length(&[(0.0, 1.0), (0.0, 4.0)]);
        let suburban = s.length(&[(0.0, 30.0), (0.0, 33.0)]);
        assert!(central < 1.0, "3 units of city centre came to {central} km");
        assert!(suburban > 2.5, "3 units of suburb came to {suburban} km");
        assert!(suburban > central * 3.0, "{suburban} km of suburb against {central} km of city");
        // The ends of a profile are its own slopes exactly; in between, the
        // curve carries one into the other instead of stepping.
        assert!((s.km_per_unit(0.0) - 0.2).abs() < 1e-9);
        assert!((s.km_per_unit(50.0) - 1.0).abs() < 1e-9);
        assert!((s.real_radius(10.0) - 2.0).abs() < 1e-9, "through its own point");
    }

    /// The profile from a map that was being measured when this was written:
    /// nearly flat out to 50 units, then steep. Straight-line interpolation
    /// turned the join into a cliff.
    const LUMPY: [(f64, f64); 5] =
        [(11.0, 7.4), (25.0, 12.0), (50.0, 17.0), (78.0, 48.0), (110.0, 88.9)];

    #[test]
    fn the_curve_passes_through_every_point_it_is_given() {
        let s = profile(&LUMPY);
        for &(r, km) in s.points() {
            assert!(
                (s.real_radius(r) - km).abs() < 1e-9,
                "({r}, {km}) is not on its own curve",
            );
        }
    }

    #[test]
    fn the_profile_has_no_corner_at_a_control_point() {
        // The failure this exists to prevent: two neighbouring stretches of
        // track, one either side of a control point, measured at rates several
        // times apart with nothing on screen to explain it.
        let s = profile(&LUMPY);
        for &(r, _) in s.points().iter().skip(1) {
            let (before, after) = (s.km_per_unit(r - 0.01), s.km_per_unit(r + 0.01));
            assert!(
                (before - after).abs() < 0.05,
                "the slope steps from {before} to {after} km/unit at r = {r}",
            );
        }
    }

    #[test]
    fn a_profile_never_doubles_back_however_lumpy_it_is() {
        // Monotone interpolation, not plain cubic: a curve that overshot
        // between two points would hand back distances that shrink as the
        // track goes further out.
        let s = profile(&[(5.0, 30.0), (10.0, 31.0), (40.0, 32.0), (60.0, 90.0), (70.0, 91.0)]);
        let (mut last, mut r) = (0.0, 0.0);
        while r <= 90.0 {
            let now = s.real_radius(r);
            assert!(now >= last - 1e-12, "fell from {last} to {now} km by r = {r}");
            assert!(s.km_per_unit(r) >= -1e-12, "slope {} at r = {r}", s.km_per_unit(r));
            last = now;
            r += 0.05;
        }
    }

    #[test]
    fn a_straight_profile_stays_straight() {
        // Nothing to smooth, so the curve must not invent a bend of its own.
        let s = profile(&[(22.0, 11.0), (44.0, 22.0), (66.0, 33.0), (110.0, 55.0)]);
        for r in [0.5, 7.3, 22.0, 45.1, 99.0, 110.0, 140.0] {
            assert!((s.real_radius(r) - r * 0.5).abs() < 1e-9, "r = {r}");
            assert!((s.km_per_unit(r) - 0.5).abs() < 1e-9, "r = {r}");
        }
    }

    #[test]
    fn the_last_slope_carries_on_past_the_last_point() {
        let s = profile(&[(10.0, 10.0), (20.0, 30.0)]);
        assert!((s.real_radius(25.0) - 40.0).abs() < 1e-9, "got {}", s.real_radius(25.0));
        assert!((s.km_per_unit(100.0) - 2.0).abs() < 1e-9);
    }

    #[test]
    fn a_segment_is_measured_along_the_bend_the_warp_gives_it() {
        // A fiercely compressed centre: the first map unit is worth 10 km, so
        // track passing close by is flung outwards in the middle and the real
        // ride is much longer than the line between its ends.
        let s = profile(&[(1.0, 10.0), (50.0, 20.0)]);
        let (a, b) = ((-20.0, 1.0), (20.0, 1.0));
        let chord = apart(s.warp(a), s.warp(b));
        let along = s.length(&[a, b]);
        assert!(
            along > chord * 1.15,
            "measured {along} km along a bend whose chord is {chord} km",
        );
    }

    #[test]
    fn a_control_point_cannot_pass_its_neighbours() {
        let mut s = profile(&[(10.0, 10.0), (20.0, 20.0), (30.0, 30.0)]);
        let (r, km) = s.move_point(2, 1.0, -5.0);
        assert!(r >= 10.0 + MIN_GAP, "radius {r} slid past the point below it");
        assert!(km >= 10.0, "{km} km dipped under the point below it");
        let (r, km) = s.move_point(2, 999.0, 99.0);
        assert!(r <= 30.0 - MIN_GAP, "radius {r} slid past the point above it");
        assert!(km <= 30.0, "{km} km rose over the point above it");
        for w in s.points().windows(2) {
            assert!(w[1].0 > w[0].0 && w[1].1 >= w[0].1, "still rising: {:?}", s.points());
        }
    }

    #[test]
    fn the_last_point_may_be_dragged_up_but_not_past_the_reach() {
        let mut s = profile(&[(10.0, 10.0), (30.0, 30.0)]);
        assert_eq!(s.move_point(2, 90.0, 400.0), (30.0, 400.0), "the top is open");
        assert_eq!(s.reach(), 30.0, "the axis does not move with it");
    }

    #[test]
    fn the_origin_is_pinned_because_the_centre_is_zero_from_itself() {
        let mut s = profile(&[(10.0, 10.0)]);
        assert_eq!(s.move_point(0, 5.0, 5.0), (0.0, 0.0));
        assert_eq!(s.points()[0], (0.0, 0.0));
        assert_eq!(s.move_point(9, 5.0, 5.0), (0.0, 0.0), "and nothing off the end moves");
    }

    #[test]
    fn a_profile_is_sorted_and_never_doubles_back() {
        let s = Scale::from_points(
            (0.0, 0.0),
            vec![(30.0, 12.0), (10.0, 20.0), (10.2, 99.0), (-4.0, 3.0), (f64::NAN, 1.0)],
        );
        assert_eq!(s.points(), [(0.0, 0.0), (10.0, 20.0), (30.0, 20.0)]);
        assert_eq!(s.reach(), 30.0);
    }

    #[test]
    fn even_an_empty_profile_can_measure_something() {
        let s = profile(&[]);
        assert!(s.points().len() >= 2, "a slope needs two points");
        assert!(s.real_radius(100.0).is_finite());
        assert!(s.length(&[(0.0, 0.0), (3.0, 4.0)]) > 0.0);
    }
}
