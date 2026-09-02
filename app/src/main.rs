use leptos::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

const THUMB_WIDTH_PX: f32 = 160.0;

fn unpremultiply(data: &[u8]) -> Vec<u8> {
    let mut rgba = data.to_vec();
    for px in rgba.chunks_exact_mut(4) {
        let a = px[3] as u32;
        if a > 0 && a < 255 {
            px[0] = ((px[0] as u32 * 255) / a).min(255) as u8;
            px[1] = ((px[1] as u32 * 255) / a).min(255) as u8;
            px[2] = ((px[2] as u32 * 255) / a).min(255) as u8;
        }
    }
    rgba
}

fn render_pixmap(
    pdf_bytes: &[u8],
    page_index: usize,
    scale: f32,
) -> Result<hayro::vello_cpu::Pixmap, String> {
    use hayro::hayro_interpret::InterpreterSettings;
    use hayro::hayro_syntax::Pdf;
    use hayro::{RenderCache, RenderSettings};

    let pdf = Pdf::new(pdf_bytes.to_vec()).map_err(|e| format!("parse: {e:?}"))?;
    let page = pdf
        .pages()
        .get(page_index)
        .ok_or_else(|| format!("no page {page_index}"))?;
    let mut settings = RenderSettings::default();
    settings.bg_color = hayro::vello_cpu::color::palette::css::WHITE;
    settings.x_scale = scale;
    settings.y_scale = scale;
    Ok(hayro::render(
        page,
        &RenderCache::new(),
        &InterpreterSettings::default(),
        &settings,
    ))
}

fn paint_pixmap(
    pixmap: &hayro::vello_cpu::Pixmap,
    canvas: &web_sys::HtmlCanvasElement,
) -> Result<(), String> {
    canvas.set_width(pixmap.width() as u32);
    canvas.set_height(pixmap.height() as u32);
    let ctx = canvas
        .get_context("2d")
        .map_err(|e| format!("ctx: {e:?}"))?
        .ok_or("no 2d context")?
        .dyn_into::<web_sys::CanvasRenderingContext2d>()
        .map_err(|e| format!("ctx cast: {e:?}"))?;
    let rgba = unpremultiply(pixmap.data_as_u8_slice());
    let img = web_sys::ImageData::new_with_u8_clamped_array_and_sh(
        wasm_bindgen::Clamped(&rgba[..]),
        pixmap.width() as u32,
        pixmap.height() as u32,
    )
    .map_err(|e| format!("imagedata: {e:?}"))?;
    ctx.put_image_data(&img, 0.0, 0.0)
        .map_err(|e| format!("put_image_data: {e:?}"))
}

fn render_page_to_canvas(
    pdf_bytes: &[u8],
    page_index: usize,
    zoom: f32,
    canvas: &web_sys::HtmlCanvasElement,
) -> Result<(), String> {
    let dpr = web_sys::window()
        .map(|w| w.device_pixel_ratio())
        .unwrap_or(1.0)
        .clamp(1.0, 3.0);
    let scale = (zoom.max(0.05) as f64 * dpr) as f32;
    let pixmap = render_pixmap(pdf_bytes, page_index, scale)?;
    paint_pixmap(&pixmap, canvas)?;
    let el: &web_sys::HtmlElement = canvas.unchecked_ref();
    el.style()
        .set_property("width", &format!("{}px", pixmap.width() as f32 / dpr as f32))
        .ok();
    el.style()
        .set_property("height", &format!("{}px", pixmap.height() as f32 / dpr as f32))
        .ok();
    Ok(())
}

fn render_thumbnail(pdf_bytes: &[u8], page_index: usize) -> Result<String, String> {
    use hayro::hayro_syntax::Pdf;
    let pdf = Pdf::new(pdf_bytes.to_vec()).map_err(|e| format!("parse: {e:?}"))?;
    let page = pdf
        .pages()
        .get(page_index)
        .ok_or_else(|| format!("no page {page_index}"))?;
    let (w, _) = page.render_dimensions();
    let scale = if w > 0.0 { THUMB_WIDTH_PX / w } else { 0.25 };
    drop(pdf);
    let pixmap = render_pixmap(pdf_bytes, page_index, scale)?;

    let document = web_sys::window()
        .and_then(|w| w.document())
        .ok_or("no document")?;
    let canvas: web_sys::HtmlCanvasElement = document
        .create_element("canvas")
        .map_err(|e| format!("{e:?}"))?
        .unchecked_into();
    paint_pixmap(&pixmap, &canvas)?;
    canvas.to_data_url().map_err(|e| format!("{e:?}"))
}

fn download_bytes(bytes: &[u8], name: &str) {
    let parts = js_sys::Array::new();
    parts.push(&js_sys::Uint8Array::from(bytes));
    let mut opts = web_sys::BlobPropertyBag::new();
    opts.set_type("application/pdf");
    let Some(window) = web_sys::window() else { return };
    let Some(document) = window.document() else { return };
    let Ok(blob) = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &opts) else {
        return;
    };
    let Ok(url) = web_sys::Url::create_object_url_with_blob(&blob) else {
        return;
    };
    if let Ok(a) = document.create_element("a") {
        let a: web_sys::HtmlAnchorElement = a.unchecked_into();
        a.set_href(&url);
        a.set_download(name);
        a.click();
    }
    let _ = web_sys::Url::revoke_object_url(&url);
}

#[component]
fn App() -> impl IntoView {
    let status = RwSignal::new("Load a PDF to begin.".to_string());
    let page = RwSignal::new(0usize);
    let page_count = RwSignal::new(0usize);
    let pdf_bytes = RwSignal::new(Vec::<u8>::new());
    let filename = RwSignal::new("edited.pdf".to_string());
    let zoom = RwSignal::new(1.0f32);
    let thumbs = RwSignal::new(Vec::<String>::new());
    let canvas_ref = NodeRef::<leptos::html::Canvas>::new();

    // re-render main canvas whenever document, page, or zoom changes
    Effect::new(move || {
        let bytes = pdf_bytes.get();
        let p = page.get();
        let z = zoom.get();
        if bytes.is_empty() {
            return;
        }
        request_animation_frame(move || {
            if let Some(canvas) = canvas_ref.get() {
                let canvas_el: &web_sys::HtmlCanvasElement = &canvas;
                if let Err(e) = render_page_to_canvas(&bytes, p, z, canvas_el) {
                    status.set(format!("render error: {e}"));
                }
            }
        });
    });

    // regenerate thumbnails whenever the document changes
    Effect::new(move || {
        let bytes = pdf_bytes.get();
        if bytes.is_empty() {
            thumbs.set(vec![]);
            return;
        }
        let n = match pdf_edit_core::PdfDoc::load(&bytes) {
            Ok(d) => d.page_count(),
            Err(_) => 0,
        };
        let urls: Vec<String> = (0..n)
            .map(|i| render_thumbnail(&bytes, i).unwrap_or_default())
            .collect();
        thumbs.set(urls);
    });

    let on_file = move |ev: leptos::ev::Event| {
        let input: web_sys::HtmlInputElement = event_target(&ev);
        let Some(file) = input.files().and_then(|fl: web_sys::FileList| fl.get(0)) else {
            return;
        };
        filename.set(file.name());
        status.set("Loading…".to_string());
        leptos::task::spawn_local(async move {
            match JsFuture::from(file.array_buffer()).await {
                Ok(buf) => {
                    let bytes = js_sys::Uint8Array::new(&buf).to_vec();
                    match pdf_edit_core::PdfDoc::load(&bytes) {
                        Ok(doc) => {
                            page_count.set(doc.page_count());
                            page.set(0);
                            zoom.set(1.0);
                            pdf_bytes.set(bytes);
                            status.set("Loaded.".to_string());
                        }
                        Err(e) => status.set(format!("lopdf could not load: {e}")),
                    }
                }
                Err(e) => status.set(format!("read error: {e:?}")),
            }
        });
    };

    let apply_edit = move |f: &dyn Fn(&mut pdf_edit_core::PdfDoc)| {
        let bytes = pdf_bytes.get();
        if bytes.is_empty() {
            return;
        }
        match pdf_edit_core::PdfDoc::load(&bytes) {
            Ok(mut doc) => {
                f(&mut doc);
                match doc.save() {
                    Ok(out) => {
                        let n = doc.page_count();
                        page_count.set(n);
                        page.update(|p| {
                            if *p >= n {
                                *p = n.saturating_sub(1)
                            }
                        });
                        pdf_bytes.set(out);
                    }
                    Err(e) => status.set(format!("save-after-edit failed: {e}")),
                }
            }
            Err(e) => status.set(format!("reload failed: {e}")),
        }
    };

    let set_zoom = move |f: fn(f32) -> f32| {
        zoom.update(|z| *z = f(*z).clamp(0.25, 5.0));
    };

    let on_save = move |_| {
        if pdf_bytes.get().is_empty() {
            status.set("Nothing to save yet.".to_string());
            return;
        }
        let result = pdf_edit_core::PdfDoc::load(&pdf_bytes.get()).and_then(|mut d| d.save());
        match result {
            Ok(out) => {
                status.set(format!("Saved {} ({} KB)", filename.get(), out.len() / 1024));
                download_bytes(&out, &filename.get());
            }
            Err(e) => status.set(format!("save error: {e}")),
        }
    };

    let on_merge = move |ev: leptos::ev::Event| {
        let input: web_sys::HtmlInputElement = event_target(&ev);
        let Some(file) = input.files().and_then(|fl: web_sys::FileList| fl.get(0)) else {
            return;
        };
        if pdf_bytes.get().is_empty() {
            status.set("Load a PDF first, then merge.".to_string());
            return;
        }
        status.set("Merging…".to_string());
        leptos::task::spawn_local(async move {
            match JsFuture::from(file.array_buffer()).await {
                Ok(buf) => {
                    let other = js_sys::Uint8Array::new(&buf).to_vec();
                    let merged = pdf_edit_core::PdfDoc::load(&pdf_bytes.get())
                        .and_then(|mut d| {
                            d.append(&other)?;
                            d.save()
                        });
                    match merged {
                        Ok(out) => {
                            pdf_bytes.set(out);
                            status.set("Merged.".to_string());
                        }
                        Err(e) => status.set(format!("merge failed: {e}")),
                    }
                }
                Err(e) => status.set(format!("read error: {e:?}")),
            }
        });
    };

    let extract_from = RwSignal::new(1u32);
    let extract_to = RwSignal::new(1u32);
    let on_extract = move |_| {
        if pdf_bytes.get().is_empty() {
            status.set("Load a PDF first.".to_string());
            return;
        }
        let (from, to) = (extract_from.get(), extract_to.get());
        match pdf_edit_core::PdfDoc::load(&pdf_bytes.get()).and_then(|d| d.extract(from, to)) {
            Ok(out) => {
                let name = format!("pages-{}-{}-{}", from, to, filename.get());
                status.set(format!("Extracted pages {}–{}.", from, to));
                download_bytes(&out, &name);
            }
            Err(e) => status.set(format!("extract failed: {e}")),
        }
    };

    view! {
        <main style="font-family: system-ui; max-width: 1100px; margin: 1.5rem auto; padding: 0 1rem;">
            <h1 style="margin-bottom: 0.25rem;">"pdf-edit"</h1>
            <p style="margin-top: 0; color: #555;">"Files never leave your device."</p>
            <input type="file" accept="application/pdf" on:change=on_file />
            <div style="margin: 0.75rem 0; display: flex; gap: 0.5rem; align-items: center; flex-wrap: wrap;">
                <button on:click=move |_| page.update(|p| *p = p.saturating_sub(1))>"← Prev"</button>
                <button on:click=move |_| page.update(|p| { if *p + 1 < page_count.get() { *p += 1 } })>"Next →"</button>
                <span style="border-left: 1px solid #ccc; padding-left: 0.5rem;">
                    <button on:click=move |_| set_zoom(|z| z / 1.25)>"−"</button>
                    <button on:click=move |_| set_zoom(|_| 1.0)>{move || format!("{}%", (zoom.get() * 100.0).round())}</button>
                    <button on:click=move |_| set_zoom(|z| z * 1.25)>"+"</button>
                </span>
                <button style="font-weight: 600;" on:click=on_save>"Save PDF"</button>
                <span>{move || status.get()}</span>
            </div>
            <div style="margin: 0.25rem 0 0.75rem; display: flex; gap: 0.5rem; align-items: center; flex-wrap: wrap; font-size: 14px;">
                <label style="border: 1px solid #999; border-radius: 4px; padding: 1px 8px; cursor: pointer;">
                    "Merge PDF…"
                    <input type="file" accept="application/pdf" style="display: none;" on:change=on_merge />
                </label>
                <span style="border-left: 1px solid #ccc; padding-left: 0.5rem; display: flex; gap: 0.35rem; align-items: center;">
                    "Extract pages"
                    <input type="number" min="1" style="width: 3.5rem;" prop:value=move || extract_from.get()
                        on:change=move |ev| { if let Ok(v) = event_target_value(&ev).parse() { extract_from.set(v); } } />
                    "–"
                    <input type="number" min="1" style="width: 3.5rem;" prop:value=move || extract_to.get()
                        on:change=move |ev| { if let Ok(v) = event_target_value(&ev).parse() { extract_to.set(v); } } />
                    <button on:click=on_extract>"Download"</button>
                </span>
            </div>
            <div style="display: flex; gap: 1rem; align-items: flex-start;">
                <div style="width: 190px; flex-shrink: 0; display: flex; flex-direction: column; gap: 0.75rem; max-height: 80vh; overflow-y: auto;">
                    {move || {
                        thumbs.get().into_iter().enumerate().map(|(i, url)| {
                            let num = i + 1;
                            view! {
                                <div style=move || format!(
                                    "border: 2px solid {}; border-radius: 4px; padding: 4px;",
                                    if page.get() == i { "#2463eb" } else { "#ddd" }
                                )>
                                    <img
                                        src=url
                                        style="width: 100%; display: block; cursor: pointer;"
                                        on:click=move |_| page.set(i)
                                    />
                                    <div style="display: flex; justify-content: space-between; align-items: center; font-size: 12px; margin-top: 2px;">
                                        <span>{num}</span>
                                        <span style="display: flex; gap: 2px;">
                                            <button title="Rotate 90°" on:click=move |_| apply_edit(&|d| { let _ = d.rotate_page_by(num as u32, 90); })>"↻"</button>
                                            <button title="Move up" on:click=move |_| { if num > 1 { apply_edit(&|d| { let _ = d.move_page(num as u32, num as u32 - 1); }); page.set(i - 1); } }>"↑"</button>
                                            <button title="Move down" on:click=move |_| { if num < page_count.get() { apply_edit(&|d| { let _ = d.move_page(num as u32, num as u32 + 1); }); page.set(i + 1); } }>"↓"</button>
                                            <button title="Delete page" on:click=move |_| { if page_count.get() > 1 { apply_edit(&|d| d.delete_page(num as u32)); } }>"✕"</button>
                                        </span>
                                    </div>
                                </div>
                            }
                        }).collect_view()
                    }}
                </div>
                <canvas node_ref=canvas_ref style="border: 1px solid #ccc; display: block;"></canvas>
            </div>
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

#[cfg(test)]
mod tests {
    use lopdf::{dictionary, Document, Object, Stream};

    fn make_pdf_with_rect() -> Vec<u8> {
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 200.into(), 200.into()],
        });
        // fill a 100x100 black rect
        let content_id = doc.add_object(Stream::new(
            dictionary! {},
            b"0 0 0 rg 50 50 100 100 re f".to_vec(),
        ));
        doc.get_dictionary_mut(page_id)
            .unwrap()
            .set("Contents", content_id);
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![page_id.into()],
                "Count" => 1,
            }),
        );
        let catalog_id = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        doc.trailer.set("Root", catalog_id);
        let mut out = Vec::new();
        doc.save_to(&mut out).unwrap();
        out
    }

    #[test]
    fn hayro_renders_vector_content() {
        use hayro::hayro_interpret::InterpreterSettings;
        use hayro::hayro_syntax::Pdf;
        use hayro::{RenderCache, RenderSettings};

        let bytes = make_pdf_with_rect();
        let pdf = Pdf::new(bytes).unwrap();
        let page = pdf.pages().get(0).unwrap();
        let pixmap = hayro::render(
            page,
            &RenderCache::new(),
            &InterpreterSettings::default(),
            &RenderSettings::default(),
        );
        assert_eq!(pixmap.width(), 200);
        assert_eq!(pixmap.height(), 200);
        let data = pixmap.data_as_u8_slice();
        let has_dark = data.chunks_exact(4).any(|px| px[0] < 128);
        assert!(has_dark, "expected the black rect to be rasterized");
    }
}
