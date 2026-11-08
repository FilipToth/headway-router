//! What kind of service a line runs, and how fast that kind of service goes.
//!
//! A Tennessine map says almost nothing about speed. It says how a line is
//! *drawn* — plain, white-striped, dashed — which the parser reads as
//! [`Mode`], and that is a decent first guess at metro, tram and commuter
//! rail. It is only a guess: the same drawing covers a 50 km/h tram and a
//! 160 km/h express, and an author may well draw a high-speed line like any
//! other. So the categories here start from the drawing and are then the
//! user's to rename, add to, and drag lines between.
//!
//! A category carries the numbers a run between two stops depends on: how
//! fast the service can go, how hard it speeds up, how hard it slows down, and
//! how often it comes. [`Category::run_time`] is what the first three are for
//! — the time to cover a distance from a standing start to a stand — and it is
//! deliberately the only place that arithmetic lives, so the panel's readouts
//! and the journey times cannot drift apart.
//!
//! Frequency is a category's by default and a line's when it says otherwise,
//! because lines of one kind do differ: two metros on the same stock, one
//! every three minutes and one every twelve, are the same category in every
//! way but the wait. What that wait costs is [`crate::timing`]'s business.
//!
//! Two figures here are the journey's rather than any one line's — the walk
//! across an interchange ([`Services::walk`]) and, still fixed,
//! [`crate::timing::DWELL`]. The walk is kept with the categories because that
//! is where it is edited, next to the frequencies it is added to.

use crate::network::{LineId, Mode, Network};

/// Slowest and fastest a category may be set to, in km/h. Wide enough for a
/// funicular at one end and a maglev at the other.
pub const MIN_SPEED: f64 = 5.0;
pub const MAX_SPEED: f64 = 600.0;
/// Gentlest and harshest acceleration or braking, in m/s². The upper end is
/// past anything a standing passenger would forgive.
pub const MIN_RATE: f64 = 0.1;
pub const MAX_RATE: f64 = 4.0;
/// Seconds spent walking across an interchange, and the most that can be set.
/// Flat, because nothing in a Tennessine file says how big a station is.
pub const DEFAULT_WALK: f64 = 60.0;
pub const MAX_WALK: f64 = 900.0;
/// Closest together and furthest apart two services may run, in minutes.
pub const MIN_HEADWAY: f64 = 0.5;
pub const MAX_HEADWAY: f64 = 120.0;
/// Longest category name kept. Names are display text, not identifiers.
pub const MAX_NAME: usize = 40;

/// One kind of service: everything the lines in it have in common.
#[derive(Debug, Clone, PartialEq)]
pub struct Category {
    pub name: String,
    /// km/h.
    pub top_speed: f64,
    /// m/s².
    pub accel: f64,
    /// m/s².
    pub brake: f64,
    /// Minutes between services, which is what a wait is made of. A line in
    /// the category may say otherwise; see [`Services::headway_of`].
    pub headway: f64,
}

impl Category {
    /// Top speed in metres per second, which is what the arithmetic wants.
    pub fn top_ms(&self) -> f64 {
        self.top_speed / 3.6
    }

    /// Seconds from a stand to top speed.
    pub fn time_to_top(&self) -> f64 {
        self.top_ms() / self.accel
    }

    /// Metres covered reaching top speed.
    pub fn distance_to_top(&self) -> f64 {
        self.top_ms().powi(2) / (2.0 * self.accel)
    }

    /// Seconds to cover `km` from a standing start to a stand.
    ///
    /// Speeds up, runs, slows down. On a short enough hop there is no running:
    /// the service brakes the moment it stops accelerating, and never reaches
    /// its top speed at all — which is why station spacing changes a journey
    /// so much more than the top speed on the tin does.
    pub fn run_time(&self, km: f64) -> f64 {
        let metres = (km * 1000.0).max(0.0);
        let (v, a, b) = (self.top_ms(), self.accel, self.brake);
        let ramps = v * v / (2.0 * a) + v * v / (2.0 * b);
        if metres >= ramps {
            v / a + v / b + (metres - ramps) / v
        } else {
            // Peak speed where speeding up has to give way to slowing down.
            let peak = (2.0 * metres * a * b / (a + b)).sqrt();
            peak / a + peak / b
        }
    }
}

/// The categories a map's lines are sorted into.
///
/// Tied to one parsed [`Network`]: lines are held by their index in it, so a
/// `Services` outlives neither a different map nor a re-parse.
#[derive(Debug, Clone, PartialEq)]
pub struct Services {
    /// Never empty. Index 0 is the default: where a line sits until it is
    /// moved, and the one category that cannot be removed.
    categories: Vec<Category>,
    /// Which category each line is in, by line index.
    assigned: Vec<usize>,
    /// A line's own headway in minutes where it differs from its category's,
    /// by line index.
    own_headway: Vec<Option<f64>>,
    /// Seconds walking from one platform to another at a change. Not a
    /// category's and not a line's, but edited here alongside them.
    walk: f64,
}

impl Default for Services {
    /// What an app holds before a map is loaded: one category, no lines.
    fn default() -> Self {
        Self::fresh(0, name_of(Mode::Metro), Mode::Metro)
    }
}

impl Services {
    /// One default category, named `name`, with every line in it.
    pub fn fresh(lines: usize, name: &str, like: Mode) -> Self {
        Self {
            categories: vec![from_mode(name, like)],
            assigned: vec![0; lines],
            own_headway: vec![None; lines],
            walk: DEFAULT_WALK,
        }
    }

    /// Categories seeded from how the map draws its lines, with the commonest
    /// drawing as the default so most lines need never be dragged anywhere.
    ///
    /// A guess worth making: it is the only thing in the file that speaks
    /// about service at all, and it is right about trams far more often than
    /// not. Renaming or re-dragging it costs the user one gesture.
    pub fn for_network(net: &Network) -> Self {
        let mut counts: Vec<(Mode, usize)> = Vec::new();
        for line in &net.lines {
            match counts.iter_mut().find(|(m, _)| *m == line.mode) {
                Some((_, n)) => *n += 1,
                None => counts.push((line.mode, 1)),
            }
        }
        // Commonest first, and a stable order after that so two parses of one
        // map never disagree about which category is which.
        counts.sort_by_key(|(mode, n)| (std::cmp::Reverse(*n), name_of(*mode)));

        let Some(&(default, _)) = counts.first() else {
            return Self::fresh(net.lines.len(), name_of(Mode::Metro), Mode::Metro);
        };
        let mut services = Self::fresh(net.lines.len(), name_of(default), default);
        for &(mode, _) in counts.iter().skip(1) {
            let at = services.categories.len();
            services.categories.push(from_mode(name_of(mode), mode));
            for line in net.lines.iter().filter(|l| l.mode == mode) {
                services.assign(line.id, at);
            }
        }
        services
    }

    pub fn categories(&self) -> &[Category] {
        &self.categories
    }

    /// How many lines this covers, which is the map's line count.
    pub fn lines(&self) -> usize {
        self.assigned.len()
    }

    /// Which category a line is in. Out-of-range lines read as the default,
    /// so a stale id can never panic the view that is drawing it.
    pub fn category_of(&self, line: LineId) -> usize {
        self.assigned.get(line.0).copied().unwrap_or(0)
    }

    /// Every line in a category, in map order.
    pub fn lines_in(&self, category: usize) -> Vec<LineId> {
        self.assigned
            .iter()
            .enumerate()
            .filter(|(_, at)| **at == category)
            .map(|(i, _)| LineId(i))
            .collect()
    }

    /// Moves a line. A category that does not exist is ignored rather than
    /// obeyed, because the only caller is a drop target.
    pub fn assign(&mut self, line: LineId, category: usize) {
        if category < self.categories.len() {
            if let Some(at) = self.assigned.get_mut(line.0) {
                *at = category;
            }
        }
    }

    /// Adds an empty category, copying the default's numbers as a start, and
    /// returns where it landed.
    pub fn add(&mut self, name: &str) -> usize {
        let mut fresh = self.categories[0].clone();
        fresh.name = trimmed(name);
        self.categories.push(fresh);
        self.categories.len() - 1
    }

    pub fn rename(&mut self, category: usize, name: &str) {
        if let Some(c) = self.categories.get_mut(category) {
            c.name = trimmed(name);
        }
    }

    /// Removes a category, returning its lines to the default.
    ///
    /// The default itself stays: every line has to be somewhere, and a map
    /// with no categories at all could not be put back together by dragging.
    pub fn remove(&mut self, category: usize) {
        if category == 0 || category >= self.categories.len() {
            return;
        }
        self.categories.remove(category);
        for at in &mut self.assigned {
            match (*at).cmp(&category) {
                std::cmp::Ordering::Equal => *at = 0,
                std::cmp::Ordering::Greater => *at -= 1,
                std::cmp::Ordering::Less => {}
            }
        }
    }

    pub fn set_top_speed(&mut self, category: usize, kmh: f64) {
        if let Some(c) = self.categories.get_mut(category) {
            c.top_speed = sane(kmh, MIN_SPEED, MAX_SPEED, c.top_speed);
        }
    }

    pub fn set_accel(&mut self, category: usize, ms2: f64) {
        if let Some(c) = self.categories.get_mut(category) {
            c.accel = sane(ms2, MIN_RATE, MAX_RATE, c.accel);
        }
    }

    pub fn set_brake(&mut self, category: usize, ms2: f64) {
        if let Some(c) = self.categories.get_mut(category) {
            c.brake = sane(ms2, MIN_RATE, MAX_RATE, c.brake);
        }
    }

    pub fn set_headway(&mut self, category: usize, minutes: f64) {
        if let Some(c) = self.categories.get_mut(category) {
            c.headway = sane(minutes, MIN_HEADWAY, MAX_HEADWAY, c.headway);
        }
    }

    /// Minutes between services on a line: its own figure if it has one, its
    /// category's otherwise.
    pub fn headway_of(&self, line: LineId) -> f64 {
        self.own_headway
            .get(line.0)
            .copied()
            .flatten()
            .unwrap_or_else(|| self.categories[self.category_of(line)].headway)
    }

    /// The line's own figure, if it has one. `None` means it follows its
    /// category, including when the category is edited afterwards.
    pub fn own_headway(&self, line: LineId) -> Option<f64> {
        self.own_headway.get(line.0).copied().flatten()
    }

    /// Seconds walking across an interchange at a change, on top of the wait.
    pub fn walk(&self) -> f64 {
        self.walk
    }

    pub fn set_walk(&mut self, seconds: f64) {
        self.walk = sane(seconds, 0.0, MAX_WALK, self.walk);
    }

    /// Gives a line its own headway, or `None` to put it back on its
    /// category's.
    pub fn set_own_headway(&mut self, line: LineId, minutes: Option<f64>) {
        let had = self.own_headway(line);
        if let Some(slot) = self.own_headway.get_mut(line.0) {
            *slot = minutes.map(|m| sane(m, MIN_HEADWAY, MAX_HEADWAY, had.unwrap_or(m)));
        }
    }
}

/// A number from a text field: clamped into range, and refused outright if it
/// is not a number, since a NaN speed would silently poison every time on the
/// map.
fn sane(v: f64, low: f64, high: f64, had: f64) -> f64 {
    match v.is_finite() {
        true => v.clamp(low, high),
        false => had,
    }
}

fn trimmed(name: &str) -> String {
    name.chars().take(MAX_NAME).collect()
}

fn name_of(mode: Mode) -> &'static str {
    match mode {
        Mode::Metro => "Metro",
        Mode::Tram => "Tram",
        Mode::CommuterRail => "Commuter rail",
    }
}

/// Starting numbers for a kind of service. Ordinary figures for ordinary
/// stock, and every one of them is meant to be edited.
fn from_mode(name: &str, mode: Mode) -> Category {
    let (top_speed, accel, brake, headway) = match mode {
        Mode::Metro => (80.0, 1.0, 1.2, 5.0),
        Mode::Tram => (50.0, 1.2, 1.5, 8.0),
        Mode::CommuterRail => (140.0, 0.7, 0.9, 20.0),
    };
    Category { name: trimmed(name), top_speed, accel, brake, headway }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn services() -> Services {
        Services::fresh(5, "Metro", Mode::Metro)
    }

    #[test]
    fn every_line_starts_in_the_default_category() {
        let s = services();
        assert_eq!(s.categories().len(), 1);
        assert_eq!(s.lines(), 5);
        for i in 0..5 {
            assert_eq!(s.category_of(LineId(i)), 0);
        }
        assert_eq!(s.lines_in(0).len(), 5);
    }

    #[test]
    fn a_line_goes_where_it_is_dragged() {
        let mut s = services();
        let rail = s.add("Commuter rail");
        s.assign(LineId(3), rail);
        assert_eq!(s.category_of(LineId(3)), rail);
        assert_eq!(s.lines_in(rail), vec![LineId(3)]);
        assert_eq!(s.lines_in(0).len(), 4);
        // A category that is not there is ignored, not obeyed.
        s.assign(LineId(3), 97);
        assert_eq!(s.category_of(LineId(3)), rail);
        s.assign(LineId(99), rail);
        assert_eq!(s.lines(), 5, "and neither is a line that is not there");
    }

    #[test]
    fn removing_a_category_returns_its_lines_to_the_default() {
        let mut s = services();
        let (tram, rail) = (s.add("Tram"), s.add("Commuter rail"));
        s.assign(LineId(1), tram);
        s.assign(LineId(2), rail);
        s.remove(tram);
        assert_eq!(s.categories().len(), 2);
        assert_eq!(s.category_of(LineId(1)), 0, "back to the default");
        // Everything above the gap shuffles down with its lines.
        assert_eq!(s.categories()[1].name, "Commuter rail");
        assert_eq!(s.category_of(LineId(2)), 1);
    }

    #[test]
    fn the_default_category_is_renamable_but_not_removable() {
        let mut s = services();
        s.rename(0, "Underground");
        assert_eq!(s.categories()[0].name, "Underground");
        s.remove(0);
        assert_eq!(s.categories().len(), 1, "every line has to be somewhere");
        s.remove(42);
        assert_eq!(s.categories().len(), 1);
        s.rename(42, "nowhere");
        assert_eq!(s.categories()[0].name, "Underground");
    }

    #[test]
    fn a_new_category_starts_from_the_default_so_it_is_already_usable() {
        let mut s = services();
        s.set_top_speed(0, 95.0);
        let next = s.add("High speed metro");
        assert_eq!(s.categories()[next].top_speed, 95.0);
        assert_eq!(s.categories()[next].name, "High speed metro");
        assert!(s.lines_in(next).is_empty(), "and empty until something is dragged in");
    }

    #[test]
    fn a_speed_a_train_could_not_do_is_clamped_not_stored() {
        let mut s = services();
        s.set_top_speed(0, 9_000.0);
        assert_eq!(s.categories()[0].top_speed, MAX_SPEED);
        s.set_top_speed(0, -4.0);
        assert_eq!(s.categories()[0].top_speed, MIN_SPEED);
        s.set_accel(0, 0.0);
        assert_eq!(s.categories()[0].accel, MIN_RATE);
        s.set_brake(0, 11.0);
        assert_eq!(s.categories()[0].brake, MAX_RATE);
        // A field mid-edit can hand us nonsense; the old value survives it.
        s.set_top_speed(0, 120.0);
        s.set_top_speed(0, f64::NAN);
        assert_eq!(s.categories()[0].top_speed, 120.0);
    }

    #[test]
    fn a_line_waits_as_often_as_its_category_until_it_says_otherwise() {
        let mut s = services();
        let rail = s.add("Commuter rail");
        s.set_headway(rail, 20.0);
        s.assign(LineId(2), rail);
        assert_eq!(s.headway_of(LineId(0)), 5.0, "the default category's");
        assert_eq!(s.headway_of(LineId(2)), 20.0, "its own category's");
        assert_eq!(s.own_headway(LineId(2)), None, "but nothing of its own");

        // A line that runs more often than its kind usually does.
        s.set_own_headway(LineId(2), Some(7.5));
        assert_eq!(s.headway_of(LineId(2)), 7.5);
        assert_eq!(s.own_headway(LineId(2)), Some(7.5));
        // Editing the category no longer reaches it.
        s.set_headway(rail, 30.0);
        assert_eq!(s.headway_of(LineId(2)), 7.5);
        // Until it is given back.
        s.set_own_headway(LineId(2), None);
        assert_eq!(s.headway_of(LineId(2)), 30.0);

        // And a line keeps its own figure wherever it is dragged.
        s.set_own_headway(LineId(1), Some(2.0));
        s.assign(LineId(1), rail);
        assert_eq!(s.headway_of(LineId(1)), 2.0);
    }

    #[test]
    fn a_frequency_no_railway_could_keep_is_clamped() {
        let mut s = services();
        s.set_headway(0, 0.0);
        assert_eq!(s.categories()[0].headway, MIN_HEADWAY);
        s.set_headway(0, 10_000.0);
        assert_eq!(s.categories()[0].headway, MAX_HEADWAY);
        s.set_own_headway(LineId(0), Some(-3.0));
        assert_eq!(s.headway_of(LineId(0)), MIN_HEADWAY);
        s.set_own_headway(LineId(0), Some(f64::INFINITY));
        assert_eq!(s.headway_of(LineId(0)), MIN_HEADWAY, "nonsense leaves what was there");
    }

    #[test]
    fn the_walk_across_an_interchange_is_a_setting_of_its_own() {
        let mut s = services();
        assert_eq!(s.walk(), DEFAULT_WALK);
        s.set_walk(150.0);
        assert_eq!(s.walk(), 150.0);
        // A station big enough to take a quarter of an hour to cross is not
        // one this model has anything to say about.
        s.set_walk(10_000.0);
        assert_eq!(s.walk(), MAX_WALK);
        s.set_walk(-30.0);
        assert_eq!(s.walk(), 0.0, "a cross-platform change is a real thing");
        s.set_walk(f64::NAN);
        assert_eq!(s.walk(), 0.0, "and nonsense leaves what was there");
    }

    #[test]
    fn a_name_cannot_grow_without_end() {
        let mut s = services();
        s.rename(0, &"x".repeat(500));
        assert_eq!(s.categories()[0].name.chars().count(), MAX_NAME);
    }

    #[test]
    fn a_long_run_speeds_up_runs_and_slows_down() {
        // 72 km/h is 20 m/s: 20 s and 200 m to get there at 1 m/s², 10 s and
        // 100 m to stop at 2 m/s². Ten km leaves 9.7 km in the middle.
        let c = Category { name: "Metro".into(), top_speed: 72.0, accel: 1.0, brake: 2.0, headway: 5.0 };
        assert!((c.time_to_top() - 20.0).abs() < 1e-9);
        assert!((c.distance_to_top() - 200.0).abs() < 1e-9);
        let want = 20.0 + 10.0 + 9_700.0 / 20.0;
        assert!((c.run_time(10.0) - want).abs() < 1e-9, "got {}", c.run_time(10.0));
    }

    #[test]
    fn a_short_hop_never_reaches_top_speed() {
        let c = Category { name: "Metro".into(), top_speed: 72.0, accel: 1.0, brake: 2.0, headway: 5.0 };
        // 300 m is exactly the two ramps, so it touches top speed and no more.
        let touching = c.run_time(0.3);
        assert!((touching - 30.0).abs() < 1e-9, "got {touching}");
        // Half of that cannot be done at an average of the same speed.
        let short = c.run_time(0.15);
        assert!(short > touching / 2.0, "{short} s for half the distance");
        assert!(short < touching, "{short} s against {touching} s");
        assert_eq!(c.run_time(0.0), 0.0);
        assert_eq!(c.run_time(-5.0), 0.0, "a negative distance is no distance");
    }

    #[test]
    fn the_run_is_continuous_where_the_two_cases_meet() {
        // The seam between "reaches top speed" and "does not" is where a
        // mistake would hide, so it is walked across.
        let c = Category { name: "Metro".into(), top_speed: 80.0, accel: 1.0, brake: 1.2, headway: 5.0 };
        let seam = (c.distance_to_top() + c.top_ms().powi(2) / (2.0 * c.brake)) / 1000.0;
        let (just_under, just_over) = (c.run_time(seam - 1e-6), c.run_time(seam + 1e-6));
        assert!((just_under - just_over).abs() < 1e-3, "{just_under} vs {just_over}");
        // And it only ever takes longer to go further.
        let mut last = 0.0;
        let mut km = 0.0;
        while km < 20.0 {
            let now = c.run_time(km);
            assert!(now >= last, "{now} s at {km} km after {last} s");
            last = now;
            km += 0.01;
        }
    }

    #[test]
    fn a_fast_service_only_wins_once_the_stops_are_far_enough_apart() {
        let metro = from_mode("Metro", Mode::Metro);
        let rail = from_mode("Commuter rail", Mode::CommuterRail);
        // Stops close together and the gentler acceleration decides it: over a
        // kilometre the commuter train is still getting going and the metro,
        // already at its own top speed, is gone. A top speed on the tin says
        // nothing until there is room to use it.
        for km in [0.2, 0.5, 1.0] {
            assert!(
                rail.run_time(km) > metro.run_time(km),
                "over {km} km: rail {} s, metro {} s",
                rail.run_time(km),
                metro.run_time(km),
            );
        }
        // Give it room and the top speed is what it is for.
        for km in [3.0, 10.0] {
            assert!(
                rail.run_time(km) < metro.run_time(km),
                "over {km} km: rail {} s, metro {} s",
                rail.run_time(km),
                metro.run_time(km),
            );
        }
    }
}
