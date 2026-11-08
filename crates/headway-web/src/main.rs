use headway_core::service::{
    MAX_HEADWAY, MAX_RATE, MAX_SPEED, MAX_WALK, MIN_HEADWAY, MIN_RATE, MIN_SPEED,
};
use headway_core::{
    fade, fastest, parse, root_inner_range, tag, trace, Category, LineId, Network, Scale, Services,
    StationId, Timing, ViewBox,
};
use leptos::ev::{DragEvent, KeyboardEvent, MouseEvent, PointerEvent, WheelEvent};
use leptos::prelude::*;
use leptos::task::spawn_local;
use std::sync::Arc;
use wasm_bindgen::JsCast;
use web_sys::{Element, Event, HtmlInputElement};

/// Zoom is measured in screen pixels per grid cell, which keeps the number
/// meaningful: at 12 a cell is 12px across, so a 3-cell station gap is 36px.
/// The upper bound is well past the ~48 px/cell of an 8000px PNG export.
const MIN_ZOOM: f64 = 0.05;
const MAX_ZOOM: f64 = 400.0;
/// Leave a little air around the map when fitting it to the window.
const FIT_MARGIN: f64 = 0.94;

fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(App);
}

/// The camera over the map.
///
/// Rendered by writing the SVG's own `viewBox`, so the browser re-renders the
/// vector geometry at device resolution at every zoom level. A CSS `scale()`
/// would be cheaper per frame but rasterises the map once at its layout size
/// and stretches the bitmap, which looks terrible on a map this detailed.
#[derive(Clone, Copy, Debug, PartialEq)]
struct View {
    /// Map coordinate shown at the top-left corner of the window.
    cam_x: f64,
    cam_y: f64,
    /// Screen pixels per grid cell.
    zoom: f64,
}

impl View {
    fn fit(vb: ViewBox, (vw, vh): (f64, f64)) -> Self {
        // Clamped at the top only: a fit must always actually fit, however
        // large the map, while MIN_ZOOM governs interactive zooming out.
        let zoom = ((vw / vb.width).min(vh / vb.height) * FIT_MARGIN).min(MAX_ZOOM);
        Self {
            cam_x: vb.min_x + vb.width / 2.0 - vw / (2.0 * zoom),
            cam_y: vb.min_y + vb.height / 2.0 - vh / (2.0 * zoom),
            zoom,
        }
    }

    /// The map coordinate under a screen position.
    fn to_map(self, cx: f64, cy: f64) -> (f64, f64) {
        (self.cam_x + cx / self.zoom, self.cam_y + cy / self.zoom)
    }

    /// Zooms by `factor`, holding the map point under (`cx`, `cy`) still —
    /// the part that makes wheel zoom feel right rather than lurch.
    fn zoom_about(self, factor: f64, cx: f64, cy: f64) -> Self {
        let zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        let (mx, my) = self.to_map(cx, cy);
        Self { cam_x: mx - cx / zoom, cam_y: my - cy / zoom, zoom }
    }

    fn panned_by(self, dx: f64, dy: f64) -> Self {
        Self { cam_x: self.cam_x - dx / self.zoom, cam_y: self.cam_y - dy / self.zoom, ..self }
    }

    /// The `viewBox` showing this camera in a `vw` x `vh` window.
    fn view_box_attr(self, (vw, vh): (f64, f64)) -> String {
        format!(
            "{:.4} {:.4} {:.4} {:.4}",
            self.cam_x, self.cam_y, vw / self.zoom, vh / self.zoom
        )
    }
}

#[derive(Clone)]
struct MapDoc {
    name: String,
    /// The uploaded root `<svg>`'s contents, re-parented under ours and cut in
    /// two at the station layer so a journey can be drawn between the halves:
    /// over the track it rides, under the markers and labels that name it.
    under: String,
    over: String,
    vb: ViewBox,
    net: Arc<Network>,
}

/// One line of an itinerary, already turned into text so the view does no
/// lookups of its own.
#[derive(Clone, PartialEq)]
enum Row {
    Ride {
        line: String,
        colour: String,
        stations: usize,
        km: f64,
        seconds: f64,
        /// How often this line runs.
        every: String,
    },
    Transfer {
        at: String,
        seconds: f64,
        /// The walk and the wait, which is what the total is made of.
        detail: String,
    },
}

#[derive(Clone, PartialEq)]
struct Plan {
    rows: Vec<Row>,
    stations: usize,
    transfers: usize,
    km: f64,
    seconds: f64,
    /// Rules that fade the whole network to a pale diagram.
    css: String,
    /// The journey itself, drawn bold over that diagram.
    trace: String,
    /// A distance beside every stretch of track, for the debug toggle.
    tags: String,
}

fn window_size() -> (f64, f64) {
    let w = web_sys::window().expect("a window");
    let num = |v: Result<wasm_bindgen::JsValue, _>| v.ok().and_then(|v| v.as_f64());
    (
        num(w.inner_width()).unwrap_or(1280.0),
        num(w.inner_height()).unwrap_or(800.0),
    )
}

/// A station box you type at, with the matches listed under it.
///
/// Picking is explicit: typing clears any previous choice, so a half-typed
/// name can never leave a stale journey on screen.
#[component]
fn StationSearch(
    placeholder: &'static str,
    net: Arc<Network>,
    selected: RwSignal<Option<StationId>>,
) -> impl IntoView {
    const HINTS: usize = 7;

    let query = RwSignal::new(String::new());
    let open = RwSignal::new(false);
    let active = RwSignal::new(0usize);

    let hints = Memo::new(move |_| {
        let typed = query.get();
        net.search(&typed, HINTS)
            .into_iter()
            .map(|id| (id, net.label(id)))
            .collect::<Vec<_>>()
    });

    let choose = move |id: StationId, name: String| {
        selected.set(Some(id));
        query.set(name);
        open.set(false);
    };

    let on_input = move |ev| {
        query.set(event_target_value(&ev));
        selected.set(None);
        active.set(0);
        open.set(true);
    };

    let on_key = move |ev: KeyboardEvent| {
        let items = hints.get();
        match ev.key().as_str() {
            "ArrowDown" => {
                ev.prevent_default();
                open.set(true);
                active.update(|a| *a = (*a + 1).min(items.len().saturating_sub(1)));
            }
            "ArrowUp" => {
                ev.prevent_default();
                active.update(|a| *a = a.saturating_sub(1));
            }
            "Enter" => {
                if let Some((id, name)) = items.get(active.get()).cloned() {
                    ev.prevent_default();
                    choose(id, name);
                }
            }
            "Escape" => open.set(false),
            _ => {}
        }
    };

    view! {
        <div class="field">
            <input class="picker" type="text" placeholder=placeholder aria-label=placeholder
                   autocomplete="off" spellcheck="false"
                   prop:value=move || query.get()
                   on:input=on_input
                   on:keydown=on_key
                   on:focus=move |_| open.set(true)
                   on:blur=move |_| open.set(false) />

            <Show when=move || open.get() && !query.with(|q| q.trim().is_empty())>
                // mousedown is swallowed so the input keeps focus and the
                // click below still lands
                <ul class="hints" on:mousedown=|ev: MouseEvent| ev.prevent_default()>
                    {move || {
                        let items = hints.get();
                        if items.is_empty() {
                            return view! { <li class="hint empty">"no station by that name"</li> }
                                .into_any();
                        }
                        items.into_iter().enumerate().map(|(i, (id, name))| {
                            let picked = name.clone();
                            view! {
                                <li class="hint" class:active=move || active.get() == i
                                    on:click=move |_| choose(id, picked.clone())>
                                    {name}
                                </li>
                            }
                        }).collect_view().into_any()
                    }}
                </ul>
            </Show>
        </div>
    }
}

/// The rungs the kilometre axis snaps to: … 10, 15, 20, 25, 50, 75, 100, 150,
/// 200 … Quarters as well as halves, because a city is usually read at one of
/// them, and every decade from a campus to a continent, since nothing in a
/// Tennessine file says which one it is.
fn ladder() -> Vec<f64> {
    (-2i32..7)
        .flat_map(|e| [1.0, 1.5, 2.0, 2.5, 5.0, 7.5].map(|m| m * 10f64.powi(e)))
        .collect()
}

/// The lowest rung that still clears `km`, or the top rung if nothing does.
fn axis_ceiling(km: f64) -> f64 {
    let rungs = ladder();
    let top = rungs[rungs.len() - 1];
    rungs.into_iter().find(|rung| *rung >= km).unwrap_or(top)
}

/// The next rung up or down, for the axis buttons.
fn axis_step(km: f64, up: bool) -> f64 {
    let rungs = ladder();
    match up {
        true => rungs.iter().copied().find(|r| *r > km * 1.001),
        false => rungs.iter().copied().rev().find(|r| *r < km * 0.999),
    }
    .unwrap_or(km)
}

/// A distance as a transit app would quote it.
fn km_text(km: f64) -> String {
    match km < 1.0 {
        true => format!("{:.0} m", km * 1000.0),
        false => format!("{km:.1} km"),
    }
}

/// An axis label: enough decimals to stay distinct, no more.
fn axis_text(km: f64) -> String {
    match km {
        k if k < 1.0 => format!("{k:.2}"),
        k if k < 10.0 => format!("{k:.1}"),
        k => format!("{k:.0}"),
    }
}

/// An SVG geometry attribute. Everything drawn from Rust goes through this, so
/// a stray `1e-7` can never end up in the markup.
///
/// Four decimals, as elsewhere in the project, because the same helper writes
/// the graph's pixels and the crosshair's map units: at 400 px/cell a 1.4 px
/// pen is 0.0035 units, and rounding that to two decimals would erase it.
fn n(v: f64) -> String {
    format!("{v:.4}")
}

/// A map's starting distance model, and a kilometre axis that frames it.
fn start_scale(net: &Network, scale: RwSignal<Scale>, y_max: RwSignal<f64>) {
    let fresh = Scale::for_network(net);
    y_max.set(axis_ceiling(fresh.points().last().map_or(10.0, |p| p.1) * 1.3));
    scale.set(fresh);
}

/// The scale graph's own pixel space. The panel scales it to fit, so a pointer
/// position is divided back into these units before it means anything.
const G_W: f64 = 272.0;
const G_H: f64 = 190.0;
const PAD_L: f64 = 34.0;
const PAD_R: f64 = 8.0;
const PAD_T: f64 = 10.0;
const PAD_B: f64 = 26.0;
const PLOT_W: f64 = G_W - PAD_L - PAD_R;
const PLOT_H: f64 = G_H - PAD_T - PAD_B;
/// Bars behind the curve counting stations by how far out they are drawn.
const BUCKETS: usize = 28;
/// How near a pointer must come to pick a control point up, in graph pixels.
const GRAB: f64 = 17.0;
/// Steps the curve and the slope line are drawn in. The profile is a curve
/// through its control points, not straight lines between them, so it has to
/// be sampled to be drawn at all.
const SAMPLES: usize = 96;

fn plot_x(r: f64, reach: f64) -> f64 {
    PAD_L + (r / reach.max(1e-9)).clamp(0.0, 1.0) * PLOT_W
}
fn plot_y(km: f64, top: f64) -> f64 {
    PAD_T + PLOT_H - (km / top.max(1e-9)).clamp(0.0, 1.0) * PLOT_H
}
fn plot_r(x: f64, reach: f64) -> f64 {
    ((x - PAD_L) / PLOT_W * reach).clamp(0.0, reach)
}
fn plot_km(y: f64, top: f64) -> f64 {
    ((PAD_T + PLOT_H - y) / PLOT_H * top).clamp(0.0, top)
}

/// Where a pointer is in the graph's own coordinates.
fn graph_point(ev: &PointerEvent, el: &Element) -> (f64, f64) {
    let seen = el.get_bounding_client_rect();
    let kx = if seen.width() > 0.0 { G_W / seen.width() } else { 1.0 };
    let ky = if seen.height() > 0.0 { G_H / seen.height() } else { 1.0 };
    (
        (ev.client_x() as f64 - seen.left()) * kx,
        (ev.client_y() as f64 - seen.top()) * ky,
    )
}

/// The distance settings: where the centre of the city is, and the profile
/// that turns map units into kilometres.
///
/// It sits beside the map rather than over it on purpose. One half of the
/// setting is dragged on the map itself, and the other half is shaped against
/// the station distances drawn here as bars, so both have to be in view at
/// once.
#[component]
fn ScalePanel(
    net: Arc<Network>,
    scale: RwSignal<Scale>,
    y_max: RwSignal<f64>,
    panel: RwSignal<Option<Panel>>,
    /// Writes each leg's distance onto the map beside the leg.
    debug: RwSignal<bool>,
) -> impl IntoView {
    // Which control point the pointer is holding, if any.
    let held = RwSignal::new(None::<usize>);

    let reset = {
        let net = Arc::clone(&net);
        move |_| start_scale(&net, scale, y_max)
    };

    // How many stations are drawn at each distance from the centre. Moving the
    // centre redraws these, which is most of the point of showing them.
    let bars = {
        let net = Arc::clone(&net);
        Memo::new(move |_| {
            let s = scale.get();
            let reach = s.reach().max(1e-9);
            let mut counted = vec![0usize; BUCKETS];
            for station in &net.stations {
                let (x, y) = station.cell.centre();
                let slot = (x - s.centre.0).hypot(y - s.centre.1) / reach * BUCKETS as f64;
                // Dragging the centre can push stations past the profile's
                // reach; the curve extrapolates for them, the graph omits them.
                if (0.0..BUCKETS as f64).contains(&slot) {
                    counted[slot as usize] += 1;
                }
            }
            counted
        })
    };

    let grab = move |ev: PointerEvent| {
        let Some(el) = ev.current_target().and_then(|t| t.dyn_into::<Element>().ok()) else {
            return;
        };
        let (x, y) = graph_point(&ev, &el);
        let (s, top) = (scale.get(), y_max.get());
        let mut nearest: Option<(usize, f64)> = None;
        // Point 0 is skipped, not clamped: the centre is no distance from
        // itself, so there is nothing there to drag.
        for (i, (r, km)) in s.points().iter().enumerate().skip(1) {
            let away = (plot_x(*r, s.reach()) - x).hypot(plot_y(*km, top) - y);
            if away <= GRAB && nearest.is_none_or(|(_, best)| away < best) {
                nearest = Some((i, away));
            }
        }
        if let Some((i, _)) = nearest {
            let _ = el.set_pointer_capture(ev.pointer_id());
            held.set(Some(i));
        }
    };

    let drag = move |ev: PointerEvent| {
        let Some(i) = held.get() else { return };
        let Some(el) = ev.current_target().and_then(|t| t.dyn_into::<Element>().ok()) else {
            return;
        };
        let (x, y) = graph_point(&ev, &el);
        let top = y_max.get();
        scale.update(|s| {
            let reach = s.reach();
            s.move_point(i, plot_r(x, reach), plot_km(y, top));
        });
    };

    let release = move |_: PointerEvent| held.set(None);

    let sampled = move |height: f64, of: &dyn Fn(&Scale, f64) -> f64| {
        let s = scale.get();
        let reach = s.reach();
        let mut d = String::new();
        for i in 0..=SAMPLES {
            let r = reach * i as f64 / SAMPLES as f64;
            let step = if i == 0 { 'M' } else { 'L' };
            d.push_str(&format!("{step}{} {} ", n(plot_x(r, reach)), n(plot_y(of(&s, r), height))));
        }
        d
    };
    let curve = move || sampled(y_max.get(), &|s, r| s.real_radius(r));

    // The slope of the profile, drawn as well as the profile itself. The slope
    // is what actually converts a piece of track into a distance, and it is
    // not what the eye reads off a curve: a gentle-looking bend can be a step
    // from one rate to several times it, which shows up on the map as two
    // neighbouring sections measured wildly apart. Drawn against its own peak
    // rather than the kilometre axis, since it is a rate, not a distance.
    let peak = Memo::new(move |_| {
        let s = scale.get();
        let reach = s.reach();
        (0..=SAMPLES)
            .map(|i| s.km_per_unit(reach * i as f64 / SAMPLES as f64))
            .fold(0.0_f64, f64::max)
    });
    let slope_line = move || sampled(peak.get().max(1e-9), &|s, r| s.km_per_unit(r));

    view! {
        <div class="panel">
            <div class="panel-head">
                <span class="panel-title">"Distance"</span>
                <button class="btn btn-quiet" on:click=move |_| panel.set(None)>"Done"</button>
            </div>

            <p class="panel-note">
                "Nothing in the file says how far anything is, and a drawn map is stretched \
                 at the centre. So: put the centre where the city's is, then draw how far \
                 out a station really is against how far out it is drawn."
            </p>

            <div class="row">
                <span class="row-label">"Centre"</span>
                <output class="readout">
                    {move || {
                        let (x, y) = scale.get().centre;
                        format!("{x:.1}, {y:.1}")
                    }}
                </output>
                <button class="btn btn-quiet" title="Back to the middle of the stations"
                        on:click=reset>"Reset"</button>
            </div>
            <p class="panel-note">"Drag the crosshair on the map to move it."</p>

            <svg class="graph" viewBox=format!("0 0 {G_W} {G_H}") role="img"
                 aria-label="Real distance from the centre against drawn distance from it"
                 on:pointerdown=grab
                 on:pointermove=drag
                 on:pointerup=release
                 on:pointercancel=release>

                <rect class="plot" x=n(PAD_L) y=n(PAD_T) width=n(PLOT_W) height=n(PLOT_H) />

                {move || {
                    let counted = bars.get();
                    let tallest = counted.iter().copied().max().unwrap_or(0).max(1) as f64;
                    let w = PLOT_W / BUCKETS as f64;
                    counted.into_iter().enumerate().map(|(i, c)| {
                        let h = c as f64 / tallest * PLOT_H * 0.3;
                        view! {
                            <rect class="bar" x=n(PAD_L + i as f64 * w)
                                  y=n(PAD_T + PLOT_H - h)
                                  width=n((w - 1.0).max(0.5)) height=n(h) />
                        }
                    }).collect_view()
                }}

                {(0..=4).map(|i| {
                    let y = PAD_T + PLOT_H * (1.0 - i as f64 / 4.0);
                    view! {
                        <line class="grid" x1=n(PAD_L) y1=n(y) x2=n(PAD_L + PLOT_W) y2=n(y) />
                    }
                }).collect_view()}

                {move || [0.0, 0.5, 1.0].map(|f| {
                    let y = PAD_T + PLOT_H * (1.0 - f);
                    view! {
                        <text class="tick" x=n(PAD_L - 5.0) y=n(y + 3.0) text-anchor="end">
                            {axis_text(y_max.get() * f)}
                        </text>
                    }
                }).into_iter().collect_view()}

                {move || {
                    let reach = scale.get().reach();
                    [(0.0, "start"), (0.5, "middle"), (1.0, "end")].map(|(f, anchor)| {
                        view! {
                            <text class="tick" x=n(PAD_L + PLOT_W * f)
                                  y=n(PAD_T + PLOT_H + 12.0) text-anchor=anchor>
                                {format!("{:.0}", reach * f)}
                            </text>
                        }
                    }).into_iter().collect_view()
                }}

                <text class="axis-name" x=n(PAD_L + PLOT_W / 2.0) y=n(G_H - 2.0)
                      text-anchor="middle">"map units from centre"</text>
                <text class="axis-name" x="7" y=n(PAD_T + PLOT_H / 2.0) text-anchor="middle"
                      transform=format!("rotate(-90 7 {})", n(PAD_T + PLOT_H / 2.0))>
                    "km from centre"
                </text>

                <path class="slope" d=slope_line fill="none" />
                <path class="curve" d=curve fill="none" />

                {move || {
                    let s = scale.get();
                    let (reach, top) = (s.reach(), y_max.get());
                    s.points().iter().enumerate().map(|(i, (r, km))| {
                        view! {
                            <circle class="knob" class:pinned=i == 0
                                    class:held=move || held.get() == Some(i)
                                    cx=n(plot_x(*r, reach)) cy=n(plot_y(*km, top))
                                    r=if i == 0 { "3" } else { "4.5" } />
                        }
                    }).collect_view()
                }}
            </svg>

            <div class="row">
                <span class="row-label">"Axis"</span>
                <output class="readout">
                    {move || format!("0 \u{2013} {} km", axis_text(y_max.get()))}
                </output>
                <button class="btn btn-quiet" title="Shorter axis" on:click=move |_| {
                    // Never below the curve: a point dragged off the top of the
                    // graph could not be dragged back.
                    let top = scale.with(|s| s.points().last().map_or(0.0, |p| p.1));
                    y_max.update(|v| *v = axis_step(*v, false).max(axis_ceiling(top)));
                }>"\u{2212}"</button>
                <button class="btn btn-quiet" title="Taller axis"
                        on:click=move |_| y_max.update(|v| *v = axis_step(*v, true))>"+"</button>
            </div>

            <p class="panel-note">
                {move || {
                    let s = scale.get();
                    format!(
                        "A map unit is {} at the centre and {} at the edge. The furthest \
                         station is drawn {:.0} units out, which makes it {} away.",
                        km_text(s.km_per_unit(0.0)),
                        km_text(s.km_per_unit(s.reach())),
                        s.reach(),
                        km_text(s.real_radius(s.reach())),
                    )
                }}
            </p>
            <p class="panel-note">
                {move || format!(
                    "Dashed: that rate across the map, peaking at {} per unit. Where it \
                     climbs, every section of track out there measures longer — so a steep \
                     stretch of it is what makes two neighbouring sections disagree.",
                    km_text(peak.get()),
                )}
            </p>

            <div class="rule"></div>
            <label class="check">
                <input type="checkbox" prop:checked=move || debug.get()
                       on:change=move |ev| debug.set(event_target_checked(&ev)) />
                <span>"Label every section between stations on the map"</span>
            </label>
        </div>
    }
}

/// Which settings panel is open, if any. One at a time: they share the corner,
/// and the map has to stay visible behind whichever it is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Panel {
    Scale,
    Service,
}

/// A journey as a transit app gives it: minutes, because nobody plans around
/// the seconds.
fn mins_text(s: f64) -> String {
    let mins = (s.max(0.0) / 60.0).round() as u64;
    match mins {
        0 => "under a minute".to_string(),
        1..=59 => format!("{mins} min"),
        _ => format!("{} h {:02} min", mins / 60, mins % 60),
    }
}

/// How often a service runs, said the way a timetable says it: a two-minute
/// headway is "every 90 seconds" territory, not "every 1.5 min".
fn every_text(minutes: f64) -> String {
    let seconds = (minutes.max(0.0) * 60.0).round() as u64;
    match seconds {
        60 => "every minute".to_string(),
        0..=119 => format!("every {seconds} seconds"),
        s if s % 60 == 0 => format!("every {} min", s / 60),
        s => format!("every {:.1} min", s as f64 / 60.0),
    }
}

/// A duration as a timetable would print it.
///
/// Rounded to whole seconds *before* being split, or 119.7 s prints as 1:60.
fn secs_text(s: f64) -> String {
    let whole = s.max(0.0).round() as u64;
    match whole < 60 {
        true => format!("{whole} s"),
        false => format!("{}:{:02}", whole / 60, whole % 60),
    }
}

/// Enough hues that two categories never read as one, wrapping rather than
/// running out. Only a label: a category has no colour of its own, its lines
/// keep theirs.
fn category_hue(at: usize) -> String {
    const HUES: [u16; 6] = [214, 152, 28, 330, 268, 86];
    format!("hsl({}, 62%, 46%)", HUES[at % HUES.len()])
}

/// Service categories: what kind of thing each line is, and how fast that kind
/// of thing goes.
///
/// Seeded from how the map draws its lines, so most maps open already sorted
/// and the dragging is for the cases the drawing cannot express — a high-speed
/// metro drawn like every other metro, say.
#[component]
fn ServicePanel(
    net: Arc<Network>,
    services: RwSignal<Services>,
    panel: RwSignal<Option<Panel>>,
) -> impl IntoView {
    // The line being dragged, and the category the pointer is over.
    let held = RwSignal::new(None::<LineId>);
    let over = RwSignal::new(None::<usize>);
    // Tapped rather than dragged: the same move in two gestures, because
    // HTML's drag and drop does not exist on a touch screen.
    let picked = RwSignal::new(None::<LineId>);

    // A click inside the head or the numbers is editing, not placing.
    let stop = |ev: MouseEvent| ev.stop_propagation();

    // The list is built from the *shape* of the sorting — which lines are
    // where — and not from the names and speeds in it. Those are read field by
    // field below. Otherwise a keystroke in a name would rebuild the list, and
    // the field would be destroyed under the cursor it is being typed into.
    let grouped = Memo::new(move |_| {
        let sorted = services.get();
        (0..sorted.categories().len()).map(|at| sorted.lines_in(at)).collect::<Vec<_>>()
    });

    view! {
        <div class="panel">
            <div class="panel-head">
                <span class="panel-title">"Service"</span>
                <button class="btn btn-quiet" on:click=move |_| panel.set(None)>"Done"</button>
            </div>

            <p class="panel-note">
                "Every line runs at the speed of the category it is in, and as often as \
                 it says beside its name. Categories start from the way the map draws its \
                 lines, which is all the file says about service. Drag a line onto another \
                 category — or tap the line, then tap the category."
            </p>

            {move || grouped.get().into_iter().enumerate().map(|(at, ids)| {
                let lines: Vec<(LineId, String, String)> = ids
                    .into_iter()
                    .map(|id| {
                        let line = net.line(id);
                        (id, line.name.clone(), line.colour.clone())
                    })
                    .collect();
                let empty = lines.is_empty();

                // Each reads only its own corner of the category, so only it
                // is touched when that corner changes.
                let of = move |pick: fn(&Category) -> f64| {
                    services.with(|s| s.categories().get(at).map_or(0.0, pick))
                };
                let name = move || {
                    services.with(|s| {
                        s.categories().get(at).map_or(String::new(), |c| c.name.clone())
                    })
                };
                let reach = move || {
                    services.with(|s| {
                        s.categories().get(at).map_or(String::new(), |c| {
                            format!(
                                "Reaches {:.0} km/h in {}, {}. A 1 km hop takes {}, \
                                 and changing onto one costs {} on average.",
                                c.top_speed,
                                km_text(c.distance_to_top() / 1000.0),
                                secs_text(c.time_to_top()),
                                secs_text(c.run_time(1.0)),
                                secs_text(c.headway * 60.0 / 2.0),
                            )
                        })
                    })
                };

                view! {
                    <div class="cat" class:over=move || over.get() == Some(at)
                         on:dragover=move |ev: DragEvent| {
                             // Without this the browser refuses the drop.
                             ev.prevent_default();
                             over.set(Some(at));
                         }
                         on:dragleave=move |_| if over.get() == Some(at) { over.set(None) }
                         on:drop=move |ev: DragEvent| {
                             ev.prevent_default();
                             // Our own signal normally, but read back out of
                             // the drag itself if it started somewhere else.
                             let moved = held.get().or_else(|| {
                                 ev.data_transfer()
                                     .and_then(|data| data.get_data("text/plain").ok())
                                     .and_then(|raw| raw.parse::<usize>().ok())
                                     .map(LineId)
                             });
                             if let Some(line) = moved {
                                 services.update(|s| s.assign(line, at));
                             }
                             held.set(None);
                             over.set(None);
                             picked.set(None);
                         }
                         on:click=move |_| if let Some(line) = picked.get() {
                             services.update(|s| s.assign(line, at));
                             picked.set(None);
                         }>

                        <div class="cat-head" on:click=stop>
                            <span class="cat-dot"
                                  style=format!("background:{}", category_hue(at))></span>
                            <input class="cat-name" type="text" placeholder="Category"
                                   aria-label="Category name"
                                   prop:value=name
                                   on:input=move |ev| services.update(|s| {
                                       s.rename(at, &event_target_value(&ev))
                                   }) />
                            {(at == 0).then(|| view! { <span class="badge">"default"</span> })}
                            {(at > 0).then(|| view! {
                                <button class="btn btn-quiet"
                                        title="Remove, sending its lines back to the default"
                                        on:click=move |_| services.update(|s| s.remove(at))>
                                    "\u{00d7}"
                                </button>
                            })}
                        </div>

                        <div class="nums" on:click=stop>
                            <label>"Top km/h"
                                <input class="num" type="number" step="5"
                                       min=MIN_SPEED.to_string() max=MAX_SPEED.to_string()
                                       prop:value=move || of(|c| c.top_speed)
                                       on:change=move |ev| {
                                           if let Ok(v) = event_target_value(&ev).parse() {
                                               services.update(|s| s.set_top_speed(at, v));
                                           }
                                       } />
                            </label>
                            <label>"Every min"
                                    <input class="num" type="number" step="0.5"
                                           min=MIN_HEADWAY.to_string()
                                           max=MAX_HEADWAY.to_string()
                                           title="Minutes between services. A passenger who \
                                                  turns up unannounced waits half of it."
                                           prop:value=move || of(|c| c.headway)
                                           on:change=move |ev| {
                                               if let Ok(v) = event_target_value(&ev).parse() {
                                                   services.update(|s| s.set_headway(at, v));
                                               }
                                           } />
                                </label>
                            <label>"Accel m/s²"
                                <input class="num" type="number" step="0.1"
                                       min=MIN_RATE.to_string() max=MAX_RATE.to_string()
                                       prop:value=move || of(|c| c.accel)
                                       on:change=move |ev| {
                                           if let Ok(v) = event_target_value(&ev).parse() {
                                               services.update(|s| s.set_accel(at, v));
                                           }
                                       } />
                            </label>
                            <label>"Brake m/s²"
                                <input class="num" type="number" step="0.1"
                                       min=MIN_RATE.to_string() max=MAX_RATE.to_string()
                                       prop:value=move || of(|c| c.brake)
                                       on:change=move |ev| {
                                           if let Ok(v) = event_target_value(&ev).parse() {
                                               services.update(|s| s.set_brake(at, v));
                                           }
                                       } />
                            </label>
                        </div>

                        <p class="cat-note">{reach}</p>

                        <div class="zone">
                            {lines.into_iter().map(|(id, line, colour)| {
                                let shown = line.clone();
                                let wait = format!(
                                    "How often {line} runs, in minutes. Empty follows the \
                                     category; a passenger changing onto it waits half of it.",
                                );
                                view! {
                                    <span class="line-chip" title=line
                                          class:picked=move || picked.get() == Some(id)>
                                        // Only the name drags. An input inside
                                        // a draggable element is a fight to
                                        // click into, so the handle is the
                                        // part that is not an input.
                                        <span class="chip-grab" draggable="true"
                                              on:dragstart=move |ev: DragEvent| {
                                                  held.set(Some(id));
                                                  if let Some(data) = ev.data_transfer() {
                                                      // Firefox will not start
                                                      // a drag carrying nothing.
                                                      let _ = data.set_data(
                                                          "text/plain", &id.0.to_string(),
                                                      );
                                                      data.set_effect_allowed("move");
                                                  }
                                              }
                                              on:dragend=move |_| {
                                                  held.set(None);
                                                  over.set(None);
                                              }
                                              on:click=move |ev: MouseEvent| {
                                                  ev.stop_propagation();
                                                  picked.update(|p| {
                                                      *p = (*p != Some(id)).then_some(id)
                                                  });
                                              }>
                                            <span class="chip-swatch"
                                                  style=format!("background:{colour}")></span>
                                            {shown}
                                        </span>
                                        <input class="freq" type="number" step="0.5"
                                               min=MIN_HEADWAY.to_string()
                                               max=MAX_HEADWAY.to_string()
                                               title=wait aria-label="Minutes between services"
                                               placeholder=move || {
                                                   services.with(|s| s.headway_of(id).to_string())
                                               }
                                               prop:value=move || {
                                                   services.with(|s| {
                                                       s.own_headway(id)
                                                           .map(|m| m.to_string())
                                                           .unwrap_or_default()
                                                   })
                                               }
                                               on:click=stop
                                               on:change=move |ev| {
                                                   let raw = event_target_value(&ev);
                                                   let typed = raw.trim();
                                                   if typed.is_empty() {
                                                       services.update(|s| {
                                                           s.set_own_headway(id, None)
                                                       });
                                                   } else if let Ok(v) = typed.parse() {
                                                       services.update(|s| {
                                                           s.set_own_headway(id, Some(v))
                                                       });
                                                   }
                                               } />
                                    </span>
                                }
                            }).collect_view()}
                            {empty.then(|| view! {
                                <span class="zone-empty">"drop a line here"</span>
                            })}
                        </div>
                    </div>
                }
            }).collect_view()}

            <button class="btn btn-quiet add"
                    on:click=move |_| services.update(|s| { s.add("New category"); })>
                "+ Add category"
            </button>

            <div class="rule"></div>
            <div class="row">
                <span class="row-label">"Interchange"</span>
                <input class="num walk" type="number" step="15" min="0"
                       max=MAX_WALK.to_string()
                       aria-label="Seconds walking at a change"
                       prop:value=move || services.with(Services::walk)
                       on:change=move |ev| {
                           if let Ok(v) = event_target_value(&ev).parse() {
                               services.update(|s| s.set_walk(v));
                           }
                       } />
                <span class="row-unit">"seconds of walking at every change"</span>
            </div>

            <p class="panel-note">
                "A change costs that walk and then half the headway of whatever is \
                 boarded, which is the average wait for turning up unannounced. Waiting \
                 for the very first service is not counted: that is what a departure \
                 board is for."
            </p>
        </div>
    }
}

#[component]
fn App() -> impl IntoView {
    let doc = RwSignal::new(None::<MapDoc>);
    let error = RwSignal::new(None::<String>);
    let view = RwSignal::new(View { cam_x: 0.0, cam_y: 0.0, zoom: 10.0 });
    // Some((pointer x, pointer y, view when the drag started))
    let drag = RwSignal::new(None::<(f64, f64, View)>);
    // The viewBox depends on the window, so the window has to be reactive.
    let size = RwSignal::new(window_size());
    let origin = RwSignal::new(None::<StationId>);
    let destination = RwSignal::new(None::<StationId>);
    // Replaced the moment a map loads, since a scale only describes one map.
    let scale = RwSignal::new(Scale::from_points((0.0, 0.0), Vec::new()));
    // The graph's kilometre axis, which is a view of the profile rather than
    // part of it, so it lives here and not in the `Scale`.
    let y_max = RwSignal::new(10.0);
    let services = RwSignal::new(Services::default());
    let panel = RwSignal::new(None::<Panel>);
    // Opening one panel closes the other; pressing the open one closes it.
    let toggle = move |which: Panel| panel.update(|at| *at = (*at != Some(which)).then_some(which));
    // True while the centre crosshair is being dragged across the map.
    let moving_centre = RwSignal::new(false);
    // Set from the panel; stays on when the panel closes, which is the point
    // of it — the tags are for reading the map, not the panel.
    let debug = RwSignal::new(false);

    // Re-planned whenever either end moves — and whenever the scale or the
    // service categories change, because the quickest route is now a question
    // about seconds, and both of those are part of the answer.
    let plan = Memo::new(move |_| -> Option<Result<Plan, String>> {
        let (Some(from), Some(to)) = (origin.get(), destination.get()) else {
            return None;
        };
        let (measure, sorted) = (scale.get(), services.get());
        doc.with(|held| {
            let net = &held.as_ref()?.net;
            let timing = Timing::new(net, &measure, &sorted);
            Some(match fastest(&timing, from, to) {
                Err(why) => Err(why.to_string()),
                Ok(journey) => {
                    let mut rows = Vec::new();
                    let (mut hops, mut texts) = (Vec::new(), Vec::new());
                    for (i, leg) in journey.legs.iter().enumerate() {
                        let line = net.line(leg.line);
                        rows.push(Row::Ride {
                            line: line.name.clone(),
                            colour: line.colour.clone(),
                            stations: leg.stations(),
                            km: measure.leg_km(net, leg),
                            seconds: timing.leg(leg),
                            every: every_text(sorted.headway_of(leg.line)),
                        });
                        if let Some(next) = journey.legs.get(i + 1) {
                            let wait = secs_text(timing.wait(next.line));
                            rows.push(Row::Transfer {
                                at: net.label(leg.alight()),
                                seconds: timing.boarding(next.line),
                                detail: match timing.walk > 0.0 {
                                    true => format!("{} walk · {wait} wait", secs_text(timing.walk)),
                                    false => format!("{wait} wait"),
                                },
                            });
                        }
                        // Measured stretch by stretch rather than split out of
                        // the leg, because each label states its own track.
                        for hop in leg.segments(net) {
                            texts.push(km_text(measure.length(&hop)));
                            hops.push(hop);
                        }
                    }
                    Ok(Plan {
                        rows,
                        stations: journey.stations(),
                        transfers: journey.transfers(),
                        km: measure.journey_km(net, &journey),
                        seconds: journey.seconds,
                        css: fade(net),
                        trace: trace(net, &journey),
                        tags: tag(&hops, &texts),
                    })
                }
            })
        })
    });

    // The debug overlay, which the plan has already drawn: shown or not.
    let tags = move || match debug.get() {
        false => String::new(),
        true => plan.with(|outcome| match outcome.as_ref() {
            Some(Ok(p)) => p.tags.clone(),
            _ => String::new(),
        }),
    };

    window_event_listener(leptos::ev::resize, move |_| size.set(window_size()));

    let on_file = move |ev: Event| {
        let Some(input) = ev.target().and_then(|t| t.dyn_into::<HtmlInputElement>().ok()) else {
            return;
        };
        let Some(file) = input.files().and_then(|f| f.get(0)) else {
            return;
        };
        let name = file.name();
        spawn_local(async move {
            let text = match wasm_bindgen_futures::JsFuture::from(file.text()).await {
                Ok(t) => t.as_string().unwrap_or_default(),
                Err(_) => {
                    error.set(Some("That file could not be read.".into()));
                    return;
                }
            };
            let network = match parse(&text) {
                Ok(net) => net,
                Err(problem) => {
                    error.set(Some(format!("{name} could not be read: {problem}")));
                    doc.set(None);
                    return;
                }
            };
            let Some(body) = root_inner_range(&text) else {
                error.set(Some(format!("{name} has no root <svg> element.")));
                doc.set(None);
                return;
            };
            // Clamped because the offset comes from the parser and the range
            // from a scan of the same text: a mismatch should mis-stack the
            // route layer, never panic on a slice.
            let cut = network.station_layer_at.clamp(body.start, body.end);
            let (under, over) = (text[body.start..cut].to_string(), text[cut..body.end].to_string());
            let vb = network.view_box;
            start_scale(&network, scale, y_max);
            services.set(Services::for_network(&network));
            origin.set(None);
            destination.set(None);
            error.set(None);
            size.set(window_size());
            view.set(View::fit(vb, window_size()));
            doc.set(Some(MapDoc { name, under, over, vb, net: Arc::new(network) }));
        });
    };

    let on_pointer_down = move |ev: PointerEvent| {
        if ev.button() != 0 || doc.with(Option::is_none) {
            return;
        }
        if let Some(el) = ev.current_target().and_then(|t| t.dyn_into::<Element>().ok()) {
            let _ = el.set_pointer_capture(ev.pointer_id());
        }
        drag.set(Some((ev.client_x() as f64, ev.client_y() as f64, view.get())));
    };

    let on_pointer_move = move |ev: PointerEvent| {
        if let Some((sx, sy, start)) = drag.get() {
            view.set(start.panned_by(ev.client_x() as f64 - sx, ev.client_y() as f64 - sy));
        }
    };

    let end_drag = move |_: PointerEvent| drag.set(None);

    // The crosshair the distances are measured from. It lives inside the map's
    // own svg so that it can be placed in map coordinates, which means every
    // one of these has to keep its event away from the pan handlers above.
    let take_centre = move |ev: PointerEvent| {
        if ev.button() != 0 {
            return;
        }
        ev.stop_propagation();
        if let Some(el) = ev.current_target().and_then(|t| t.dyn_into::<Element>().ok()) {
            let _ = el.set_pointer_capture(ev.pointer_id());
        }
        moving_centre.set(true);
    };
    let move_centre = move |ev: PointerEvent| {
        if !moving_centre.get() {
            return;
        }
        ev.stop_propagation();
        let at = view.get().to_map(ev.client_x() as f64, ev.client_y() as f64);
        scale.update(|s| s.centre = at);
    };
    let drop_centre = move |ev: PointerEvent| {
        if moving_centre.get() {
            ev.stop_propagation();
            moving_centre.set(false);
        }
    };

    let on_wheel = move |ev: WheelEvent| {
        if doc.with(Option::is_none) {
            return;
        }
        ev.prevent_default();
        // A trackpad pinch arrives as a wheel event with ctrlKey set and much
        // smaller deltas than a mouse notch, so it needs its own sensitivity.
        let per_unit = if ev.ctrl_key() { 0.02 } else { 0.0015 };
        let factor = (-ev.delta_y() * per_unit).exp();
        view.update(|v| *v = v.zoom_about(factor, ev.client_x() as f64, ev.client_y() as f64));
    };

    let on_dblclick = move |ev: MouseEvent| {
        if doc.with(Option::is_none) {
            return;
        }
        view.update(|v| *v = v.zoom_about(1.7, ev.client_x() as f64, ev.client_y() as f64));
    };

    let zoom_centre = move |factor: f64| {
        let (vw, vh) = size.get();
        view.update(|v| *v = v.zoom_about(factor, vw / 2.0, vh / 2.0));
    };
    let refit = move || {
        if let Some(vb) = doc.with(|d| d.as_ref().map(|d| d.vb)) {
            view.set(View::fit(vb, size.get()));
        }
    };

    view! {
        // One hidden input, driven by <label for> buttons wherever we need one.
        <input id="svg" class="file-input" type="file" accept=".svg,image/svg+xml"
               on:change=on_file />

        <div class="viewport"
             class:dragging=move || drag.with(Option::is_some)
             on:pointerdown=on_pointer_down
             on:pointermove=on_pointer_move
             on:pointerup=end_drag
             on:pointercancel=end_drag
             on:wheel=on_wheel
             on:dblclick=on_dblclick>

            {move || doc.get().map(|d| view! {
                <svg class="stage"
                     xmlns="http://www.w3.org/2000/svg"
                     viewBox=move || view.get().view_box_attr(size.get())
                     preserveAspectRatio="xMidYMid meet">
                    // Three layers rather than one, so re-planning touches only
                    // the middle one instead of re-parsing half a megabyte of
                    // map. `g` is an SVG element to Leptos, so these are built
                    // in the SVG namespace and the halves render as they did.
                    <g inner_html=d.under />
                    <g class="route" inner_html=move || {
                        plan.get().and_then(Result::ok).map(|p| p.trace).unwrap_or_default()
                    } />
                    <g inner_html=d.over />

                    // Over the station labels rather than under them, unlike
                    // the route itself: a distance a station name can hide is
                    // no use for reading off the map.
                    <g class="tags" inner_html=tags />

                    // Over everything, and only while the panel is open: this
                    // is the one time it can be moved, and it would otherwise
                    // sit on top of a station for no reason.
                    <Show when=move || panel.get() == Some(Panel::Scale)>
                        {move || {
                            let (cx, cy) = scale.get().centre;
                            let z = view.get().zoom.max(1e-6);
                            // Sized in screen pixels, so it stays a handle at
                            // any zoom rather than a dot or a flood.
                            let (ring, arm, pen) = (9.0 / z, 15.0 / z, 1.4 / z);
                            let cross = format!(
                                "M{} {}H{}M{} {}V{}",
                                n(cx - arm), n(cy), n(cx + arm),
                                n(cx), n(cy - arm), n(cy + arm),
                            );
                            view! {
                                <g class="centre" class:moving=move || moving_centre.get()>
                                    // Twice over: white underneath so it reads
                                    // against whatever line it lands on.
                                    <path class="halo" d=cross.clone()
                                          stroke-width=n(pen * 3.5) />
                                    <circle class="halo" cx=n(cx) cy=n(cy) r=n(ring)
                                            stroke-width=n(pen * 3.5) />
                                    <path class="ink" d=cross stroke-width=n(pen) />
                                    <circle class="ink" cx=n(cx) cy=n(cy) r=n(ring)
                                            stroke-width=n(pen) />
                                    <circle class="grab" cx=n(cx) cy=n(cy) r=n(arm)
                                            on:pointerdown=take_centre
                                            on:pointermove=move_centre
                                            on:pointerup=drop_centre
                                            on:pointercancel=drop_centre />
                                </g>
                            }
                        }}
                    </Show>
                </svg>
            })}
        </div>

        // Rebuilt per map, which also clears the pickers when one is loaded.
        {move || doc.get().map(|d| {
            view! {
                <div class="topleft">
                    <div class="chip">
                        <span class="name">{d.name.clone()}</span>
                        <label for="svg" class="btn btn-quiet">"Change map"</label>
                    </div>

                    <div class="planner">
                        <StationSearch placeholder="From" net=Arc::clone(&d.net) selected=origin />
                        <StationSearch placeholder="To" net=Arc::clone(&d.net) selected=destination />

                        {move || plan.get().map(|outcome| match outcome {
                            Err(why) => view! { <p class="plan-note">{why}</p> }.into_any(),
                            Ok(p) => view! {
                                // Global, so it reaches the map; mounted with
                                // the itinerary so it comes and goes with it.
                                <style>{p.css}</style>
                                <div class="itinerary">
                                    <p class="plan-time">{mins_text(p.seconds)}</p>
                                    <p class="plan-note">{format!(
                                        "{} · {} station{} · {} transfer{}",
                                        km_text(p.km),
                                        p.stations, if p.stations == 1 { "" } else { "s" },
                                        p.transfers, if p.transfers == 1 { "" } else { "s" },
                                    )}</p>
                                    {p.rows.into_iter().map(|row| match row {
                                        Row::Ride {
                                            line, colour, stations, km, seconds, every,
                                        } => view! {
                                            <div class="ride">
                                                <span class="swatch"
                                                      style=format!("background:{colour}")></span>
                                                <div class="ride-body">
                                                    <div class="ride-top">
                                                        <span class="ride-line">{line}</span>
                                                        <span class="ride-time">
                                                            {secs_text(seconds)}
                                                        </span>
                                                    </div>
                                                    <div class="ride-sub">{format!(
                                                        "{stations} station{} · {} · {every}",
                                                        if stations == 1 { "" } else { "s" },
                                                        km_text(km),
                                                    )}</div>
                                                </div>
                                            </div>
                                        }.into_any(),
                                        Row::Transfer { at, seconds, detail } => view! {
                                            <div class="transfer">
                                                <div class="transfer-top">
                                                    <span>{format!("transfer at {at}")}</span>
                                                    <span class="transfer-time">
                                                        {secs_text(seconds)}
                                                    </span>
                                                </div>
                                                <div class="transfer-sub">{detail}</div>
                                            </div>
                                        }.into_any(),
                                    }).collect_view()}
                                </div>
                            }.into_any(),
                        })}
                    </div>
                </div>
            }
        })}

        <Show when=move || doc.with(Option::is_some)>

            <div class="controls">
                <output class="readout">
                    {move || format!("{:.0} px/cell", view.get().zoom)}
                </output>
                <button class="btn" title="Zoom in" on:click=move |_| zoom_centre(1.5)>"+"</button>
                <button class="btn" title="Zoom out" on:click=move |_| zoom_centre(1.0 / 1.5)>"\u{2212}"</button>
                <button class="btn btn-wide" title="Fit map to window"
                        on:click=move |_| refit()>"Fit"</button>
                <button class="btn btn-wide" title="Set what a map unit is worth"
                        class:on=move || panel.get() == Some(Panel::Scale)
                        on:click=move |_| toggle(Panel::Scale)>"Scale"</button>
                <button class="btn btn-wide" title="Set what each line is and how fast it runs"
                        class:on=move || panel.get() == Some(Panel::Service)
                        on:click=move |_| toggle(Panel::Service)>"Service"</button>
            </div>

            {move || doc.get().map(|d| match panel.get() {
                Some(Panel::Scale) => view! {
                    <ScalePanel net=Arc::clone(&d.net) scale=scale y_max=y_max
                                panel=panel debug=debug />
                }.into_any(),
                Some(Panel::Service) => view! {
                    <ServicePanel net=Arc::clone(&d.net) services=services panel=panel />
                }.into_any(),
                None => ().into_any(),
            })}
        </Show>

        <Show when=move || doc.with(Option::is_none)>
            <div class="welcome">
                <div class="card">
                    <h1>"Headway"</h1>
                    <p class="sub">
                        "Routing for fantasy metro maps drawn in Tennessine's Metro Designer."
                    </p>
                    <label for="svg" class="btn btn-primary">"Open a map (.svg)"</label>
                    {move || error.get().map(|e| view! { <p class="error">{e}</p> })}
                </div>
            </div>
        </Show>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The viewBox of the Muvit Amandleng map.
    const VB: ViewBox = ViewBox { min_x: -95.0, min_y: -118.0, width: 168.0, height: 184.0 };

    #[test]
    fn fit_centres_the_map_and_always_fits() {
        for (vw, vh) in [(1440.0, 900.0), (390.0, 844.0), (3840.0, 2160.0), (300.0, 300.0)] {
            for vb in [VB, ViewBox { width: 40_000.0, height: 90.0, ..VB }] {
                let v = View::fit(vb, (vw, vh));
                let (seen_w, seen_h) = (vw / v.zoom, vh / v.zoom);
                assert!(
                    seen_w >= vb.width - 1e-9 && seen_h >= vb.height - 1e-9,
                    "window shows {seen_w}x{seen_h} map units, too small for {}x{}",
                    vb.width, vb.height,
                );
                let (mx, my) = v.to_map(vw / 2.0, vh / 2.0);
                assert!((mx - (vb.min_x + vb.width / 2.0)).abs() < 1e-9, "centred horizontally");
                assert!((my - (vb.min_y + vb.height / 2.0)).abs() < 1e-9, "centred vertically");
            }
        }
    }

    #[test]
    fn zoom_holds_the_map_point_under_the_cursor() {
        let v = View { cam_x: -43.5, cam_y: 7.25, zoom: 9.0 };
        let (cx, cy) = (700.0, 380.0);
        let want = v.to_map(cx, cy);
        for factor in [1.7, 0.5, 1.0, 1.001] {
            let got = v.zoom_about(factor, cx, cy).to_map(cx, cy);
            assert!(
                (got.0 - want.0).abs() < 1e-9 && (got.1 - want.1).abs() < 1e-9,
                "factor {factor}: anchor slid from {want:?} to {got:?}",
            );
        }
    }

    #[test]
    fn zoom_clamps_at_both_ends_without_sliding() {
        let v = View { cam_x: 33.0, cam_y: 77.0, zoom: 10.0 };
        let (cx, cy) = (300.0, 200.0);
        let want = v.to_map(cx, cy);
        let deepest = v.zoom_about(1e9, cx, cy);
        let shallowest = v.zoom_about(1e-9, cx, cy);
        assert_eq!(deepest.zoom, MAX_ZOOM);
        assert_eq!(shallowest.zoom, MIN_ZOOM);
        for z in [deepest, shallowest] {
            let got = z.to_map(cx, cy);
            assert!((got.0 - want.0).abs() < 1e-9 && (got.1 - want.1).abs() < 1e-9);
        }
    }

    #[test]
    fn dragging_moves_the_map_with_the_pointer() {
        let v = View { cam_x: 0.0, cam_y: 0.0, zoom: 20.0 };
        // grab the map point at (100, 100) and drag 60px right, 40px down
        let grabbed = v.to_map(100.0, 100.0);
        let after = v.panned_by(60.0, 40.0);
        let now_at = after.to_map(160.0, 140.0);
        assert!((now_at.0 - grabbed.0).abs() < 1e-9 && (now_at.1 - grabbed.1).abs() < 1e-9,
                "the grabbed point must stay under the pointer");
        assert_eq!(after.zoom, v.zoom, "panning must not change zoom");
    }

    #[test]
    fn view_box_covers_the_window_at_the_stated_scale() {
        let v = View { cam_x: -95.0, cam_y: -118.0, zoom: 8.0 };
        assert_eq!(v.view_box_attr((1600.0, 800.0)), "-95.0000 -118.0000 200.0000 100.0000");
    }

    #[test]
    fn the_graph_reads_a_pointer_back_as_the_value_it_drew() {
        // What makes a drag feel direct: the point lands where the pointer is.
        for reach in [5.0, 110.0, 847.0] {
            for top in [1.0, 200.0] {
                for f in [0.0, 0.13, 0.5, 1.0] {
                    let (r, km) = (reach * f, top * f);
                    let back = plot_r(plot_x(r, reach), reach);
                    assert!((back - r).abs() < 1e-9, "radius {r} came back as {back}");
                    let back = plot_km(plot_y(km, top), top);
                    assert!((back - km).abs() < 1e-9, "{km} km came back as {back}");
                }
            }
        }
        // Dragged outside the plot, a point stops at its edge rather than
        // leaving the profile.
        assert_eq!(plot_r(-500.0, 110.0), 0.0);
        assert_eq!(plot_r(5_000.0, 110.0), 110.0);
        assert_eq!(plot_km(-500.0, 50.0), 50.0, "up is less y");
        assert_eq!(plot_km(5_000.0, 50.0), 0.0);
    }

    #[test]
    fn the_axis_ladder_steps_both_ways_and_stops_at_its_ends() {
        let rungs = ladder();
        let top = rungs[rungs.len() - 1];
        assert_eq!(axis_ceiling(143.0), 150.0);
        assert_eq!(axis_ceiling(150.0), 150.0, "a rung it already sits on");
        assert_eq!(axis_step(0.01, false), 0.01, "the bottom rung holds");
        assert_eq!(axis_step(top, true), top, "so does the top");
        assert_eq!(axis_ceiling(top * 10.0), top, "a ceiling past the top is the top");
        // Stepping up and back has to land where it started, or the buttons
        // would wander.
        for km in [0.5, 2.0, 20.0, 500.0] {
            assert_eq!(axis_step(axis_step(km, true), false), km, "{km}");
        }
    }

    #[test]
    fn the_axis_offers_the_quarters_a_city_is_read_at() {
        let rungs = ladder();
        for km in [25.0, 50.0, 75.0, 100.0, 150.0] {
            assert!(rungs.contains(&km), "0 \u{2013} {km} km is not on the ladder");
            assert_eq!(axis_ceiling(km), km, "{km} should need no rounding");
        }
        // And the buttons walk them in order.
        assert_eq!(axis_step(25.0, true), 50.0);
        assert_eq!(axis_step(50.0, true), 75.0);
        assert_eq!(axis_step(75.0, true), 100.0);
        assert_eq!(axis_step(100.0, true), 150.0);
        assert_eq!(axis_step(150.0, false), 100.0);
        assert_eq!(axis_step(50.0, false), 25.0);
    }

    #[test]
    fn a_frequency_is_said_the_way_a_timetable_says_it() {
        assert_eq!(every_text(0.5), "every 30 seconds");
        assert_eq!(every_text(1.0), "every minute");
        assert_eq!(every_text(1.5), "every 90 seconds");
        assert_eq!(every_text(2.0), "every 2 min");
        assert_eq!(every_text(2.5), "every 2.5 min");
        assert_eq!(every_text(20.0), "every 20 min");
        assert_eq!(every_text(120.0), "every 120 min");
    }

    #[test]
    fn a_journey_is_quoted_in_minutes_because_nobody_plans_in_seconds() {
        assert_eq!(mins_text(0.0), "under a minute");
        assert_eq!(mins_text(25.0), "under a minute");
        assert_eq!(mins_text(45.0), "1 min");
        assert_eq!(mins_text(1_989.0), "33 min");
        assert_eq!(mins_text(3_600.0), "1 h 00 min");
        assert_eq!(mins_text(5_430.0), "1 h 31 min");
    }

    #[test]
    fn a_duration_reads_as_a_timetable_prints_it() {
        assert_eq!(secs_text(0.0), "0 s");
        assert_eq!(secs_text(-3.0), "0 s", "nothing takes less than no time");
        assert_eq!(secs_text(47.4), "47 s");
        assert_eq!(secs_text(65.3), "1:05");
        assert_eq!(secs_text(600.0), "10:00");
        // The carry that a naive split gets wrong.
        assert_eq!(secs_text(59.6), "1:00");
        assert_eq!(secs_text(119.7), "2:00");
    }

    #[test]
    fn distances_read_as_metres_until_there_is_a_kilometre_of_them() {
        assert_eq!(km_text(0.2), "200 m");
        assert_eq!(km_text(0.0), "0 m");
        assert_eq!(km_text(1.0), "1.0 km");
        assert_eq!(km_text(12.34), "12.3 km");
        assert_eq!(axis_text(0.25), "0.25");
        assert_eq!(axis_text(2.0), "2.0");
        assert_eq!(axis_text(200.0), "200");
    }

    #[test]
    fn a_wheel_notch_zooms_by_a_sane_step() {
        // one mouse notch is deltaY = 100 at the sensitivity used in on_wheel
        let factor = (-(-100.0) * 0.0015f64).exp();
        assert!((1.1..1.25).contains(&factor), "a notch should be ~16%, got {factor}");
    }
}
