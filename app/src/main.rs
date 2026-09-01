use leptos::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

fn render_page_to_canvas(
    pdf_bytes: &[u8],
    page_index: usize,
    canvas: &web_sys::HtmlCanvasElement,
) -> Result<(), String> {
    use hayro::hayro_interpret::InterpreterSettings;
    use hayro::hayro_syntax::Pdf;
    use hayro::{RenderCache, RenderSettings};

    let pdf = Pdf::new(pdf_bytes.to_vec()).map_err(|e| format!("parse: {e:?}"))?;
    let page = pdf
        .pages()
        .get(page_index)
        .ok_or_else(|| format!("no page {page_index}"))?;
    let mut render_settings = RenderSettings::default();
    render_settings.bg_color = hayro::vello_cpu::color::palette::css::WHITE;
    let pixmap = hayro::render(
        page,
        &RenderCache::new(),
        &InterpreterSettings::default(),
        &render_settings,
    );

    canvas.set_width(pixmap.width() as u32);
    canvas.set_height(pixmap.height() as u32);
    let ctx = canvas
        .get_context("2d")
        .map_err(|e| format!("ctx: {e:?}"))?
        .ok_or("no 2d context")?
        .dyn_into::<web_sys::CanvasRenderingContext2d>()
        .map_err(|e| format!("ctx cast: {e:?}"))?;

    let mut rgba = pixmap.data_as_u8_slice().to_vec();
    // pixmap is premultiplied; canvas ImageData wants straight alpha
    for px in rgba.chunks_exact_mut(4) {
        let a = px[3] as u32;
        if a > 0 && a < 255 {
            px[0] = ((px[0] as u32 * 255) / a).min(255) as u8;
            px[1] = ((px[1] as u32 * 255) / a).min(255) as u8;
            px[2] = ((px[2] as u32 * 255) / a).min(255) as u8;
        }
    }
    let clamped = wasm_bindgen::Clamped(&rgba[..]);
    let img = web_sys::ImageData::new_with_u8_clamped_array_and_sh(
        clamped,
        pixmap.width() as u32,
        pixmap.height() as u32,
    )
    .map_err(|e| format!("imagedata: {e:?}"))?;
    ctx.put_image_data(&img, 0.0, 0.0)
        .map_err(|e| format!("put_image_data: {e:?}"))
}

#[component]
fn App() -> impl IntoView {
    let status = RwSignal::new("Load a PDF to begin.".to_string());
    let page = RwSignal::new(0usize);
    let page_count = RwSignal::new(0usize);
    let pdf_bytes = RwSignal::new(Vec::<u8>::new());
    let canvas_ref = NodeRef::<leptos::html::Canvas>::new();

    let render_current = move || {
        if let Some(canvas) = canvas_ref.get() {
            let canvas_el: &web_sys::HtmlCanvasElement = &canvas;
            match render_page_to_canvas(&pdf_bytes.get(), page.get(), canvas_el) {
                Ok(()) => status.set(format!("Page {} of {}", page.get() + 1, page_count.get())),
                Err(e) => status.set(format!("render error: {e}")),
            }
        }
    };

    let on_file = move |ev: leptos::ev::Event| {
        let input: web_sys::HtmlInputElement = event_target(&ev);
        let Some(file) = input.files().and_then(|fl: web_sys::FileList| fl.get(0)) else {
            return;
        };
        status.set("Loading…".to_string());
        leptos::task::spawn_local(async move {
            match JsFuture::from(file.array_buffer()).await {
                Ok(buf) => {
                    let bytes = js_sys::Uint8Array::new(&buf).to_vec();
                    match pdf_edit_core::PdfDoc::load(&bytes) {
                        Ok(doc) => {
                            page_count.set(doc.page_count());
                            page.set(0);
                            pdf_bytes.set(bytes);
                            request_animation_frame(render_current);
                        }
                        Err(e) => status.set(format!("lopdf could not load: {e}")),
                    }
                }
                Err(e) => status.set(format!("read error: {e:?}")),
            }
        });
    };

    view! {
        <main style="font-family: system-ui; max-width: 900px; margin: 2rem auto; padding: 0 1rem;">
            <h1>"pdf-edit"</h1>
            <p>"Spike build — viewer only. Files never leave your device."</p>
            <input type="file" accept="application/pdf" on:change=on_file />
            <div style="margin: 0.75rem 0;">
                <button on:click=move |_| { page.update(|p| *p = p.saturating_sub(1)); request_animation_frame(render_current); }>"← Prev"</button>
                " "
                <button on:click=move |_| { page.update(|p| { if *p + 1 < page_count.get() { *p += 1 } }); request_animation_frame(render_current); }>"Next →"</button>
                " "
                <span>{move || status.get()}</span>
            </div>
            <canvas node_ref=canvas_ref style="border: 1px solid #ccc; max-width: 100%; display: block;"></canvas>
        </main>
    }
}

fn request_animation_frame(f: impl FnOnce() + 'static) {
    let cb = wasm_bindgen::closure::Closure::once_into_js(move || f());
    let _ = web_sys::window()
        .expect("window")
        .request_animation_frame(cb.unchecked_ref());
}

fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(App);
}
