use headway_core::Cell;
use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::JsCast;
use web_sys::{Event, HtmlInputElement};

fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(App);
}

/// What we can say about an uploaded file before the parser exists.
#[derive(Clone, Debug)]
struct Loaded {
    name: String,
    bytes: usize,
    paths: usize,
    texts: usize,
    ellipses: usize,
    tennessine: bool,
}

#[component]
fn App() -> impl IntoView {
    let loaded = RwSignal::new(None::<Loaded>);

    let on_change = move |ev: Event| {
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
                Err(_) => return,
            };
            loaded.set(Some(Loaded {
                name,
                bytes: text.len(),
                paths: text.matches("<path").count(),
                texts: text.matches("<text").count(),
                ellipses: text.matches("<ellipse").count(),
                tennessine: text.contains("Tennessine"),
            }));
        });
    };

    view! {
        <main>
            <h1>"Headway"</h1>
            <p class="sub">"Routing for fantasy metro maps drawn in Tennessine's Metro Designer."</p>

            <div class="card drop">
                <label for="svg">"Upload a map exported as SVG"</label>
                <br />
                <input id="svg" type="file" accept=".svg,image/svg+xml" on:change=on_change />
            </div>

            {move || loaded.get().map(|m| view! {
                <div class="card">
                    <dl>
                        <dt>"File"</dt><dd>{m.name}</dd>
                        <dt>"Size"</dt><dd>{format!("{:.0} KB", m.bytes as f64 / 1024.0)}</dd>
                        <dt>"Tennessine export"</dt>
                        <dd>{if m.tennessine { "yes" } else { "no — unexpected format" }}</dd>
                        <dt>"<path>"</dt><dd>{m.paths}</dd>
                        <dt>"<text>"</dt><dd>{m.texts}</dd>
                        <dt>"<ellipse>"</dt><dd>{m.ellipses}</dd>
                    </dl>
                    <p class="todo">
                        "Element tallies only — headway-core does not parse the map yet."
                    </p>
                </div>
            })}

            <div class="card">
                <dl>
                    <dt>"Grid check"</dt>
                    <dd>
                        <code>{
                            // proves the core crate is linked and running in wasm:
                            // three parallel tracks in one corridor, one cell
                            let (x, y) = Cell::snap(-13.5, -2.31).centre();
                            format!("(-13.5, -2.31) snaps to cell ({x}, {y})")
                        }</code>
                    </dd>
                </dl>
            </div>
        </main>
    }
}
