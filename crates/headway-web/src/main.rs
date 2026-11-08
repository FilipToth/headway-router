use headway_core::{fade, fastest, parse, root_inner_range, trace, Network, StationId, ViewBox};
use leptos::ev::{KeyboardEvent, MouseEvent, PointerEvent, WheelEvent};
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
    Ride { line: String, colour: String, stations: usize },
    Transfer { at: String },
}

#[derive(Clone, PartialEq)]
struct Plan {
    rows: Vec<Row>,
    stations: usize,
    transfers: usize,
    /// Rules that fade the whole network to a pale diagram.
    css: String,
    /// The journey itself, drawn bold over that diagram.
    trace: String,
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

    // Re-planned whenever either end changes.
    let plan = Memo::new(move |_| -> Option<Result<Plan, String>> {
        let (Some(from), Some(to)) = (origin.get(), destination.get()) else {
            return None;
        };
        doc.with(|held| {
            let net = &held.as_ref()?.net;
            Some(match fastest(net, from, to) {
                Err(why) => Err(why.to_string()),
                Ok(journey) => {
                    let mut rows = Vec::new();
                    for (i, leg) in journey.legs.iter().enumerate() {
                        let line = net.line(leg.line);
                        rows.push(Row::Ride {
                            line: line.name.clone(),
                            colour: line.colour.clone(),
                            stations: leg.stations(),
                        });
                        if i + 1 < journey.legs.len() {
                            rows.push(Row::Transfer { at: net.label(leg.alight()) });
                        }
                    }
                    Ok(Plan {
                        rows,
                        stations: journey.stations(),
                        transfers: journey.transfers(),
                        css: fade(net),
                        trace: trace(net, &journey),
                    })
                }
            })
        })
    });

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
                                    <p class="plan-note">{format!(
                                        "{} station{} · {} transfer{}",
                                        p.stations, if p.stations == 1 { "" } else { "s" },
                                        p.transfers, if p.transfers == 1 { "" } else { "s" },
                                    )}</p>
                                    {p.rows.into_iter().map(|row| match row {
                                        Row::Ride { line, colour, stations } => view! {
                                            <div class="ride">
                                                <span class="swatch"
                                                      style=format!("background:{colour}")></span>
                                                <span class="ride-line">{line}</span>
                                                <span class="ride-count">{format!(
                                                    "{stations} station{}",
                                                    if stations == 1 { "" } else { "s" },
                                                )}</span>
                                            </div>
                                        }.into_any(),
                                        Row::Transfer { at } => view! {
                                            <div class="transfer">{format!("transfer at {at}")}</div>
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
            </div>
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
    fn a_wheel_notch_zooms_by_a_sane_step() {
        // one mouse notch is deltaY = 100 at the sensitivity used in on_wheel
        let factor = (-(-100.0) * 0.0015f64).exp();
        assert!((1.1..1.25).contains(&factor), "a notch should be ~16%, got {factor}");
    }
}
