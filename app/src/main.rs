use leptos::prelude::*;
use pdf_edit_core::annot::AnnotInfo;
use pdf_edit_core::text::{SearchHit, TextSpan};
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

const THUMB_WIDTH_PX: f32 = 160.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    View,
    Highlight,
    Underline,
    StrikeOut,
    Ink,
    Note,
}

impl Mode {
    fn label(self) -> &'static str {
        match self {
            Mode::View => "View",
            Mode::Highlight => "Highlight",
            Mode::Underline => "Underline",
            Mode::StrikeOut => "StrikeOut",
            Mode::Ink => "Ink",
            Mode::Note => "Note",
        }
    }
}

const MODES: [Mode; 6] = [
    Mode::View,
    Mode::Highlight,
    Mode::Underline,
    Mode::StrikeOut,
    Mode::Ink,
    Mode::Note,
];

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
    let opts = web_sys::BlobPropertyBag::new();
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

/// Display-space (top-left origin, points) page sizes at zoom 1.
fn page_display_sizes(pdf_bytes: &[u8]) -> Vec<(f32, f32)> {
    use hayro::hayro_syntax::Pdf;
    let Ok(pdf) = Pdf::new(pdf_bytes.to_vec()) else {
        return Vec::new();
    };
    pdf.pages()
        .iter()
        .map(|p| p.render_dimensions())
        .collect()
}

/// user→display transform for a page: [a b c d e f], x' = a·x + c·y + e.
fn page_user_to_display(pdf_bytes: &[u8], page_index: usize) -> [f64; 6] {
    use hayro::hayro_interpret::util::TransformExt;
    use hayro::hayro_syntax::Pdf;
    let fallback = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
    let Ok(pdf) = Pdf::new(pdf_bytes.to_vec()) else {
        return fallback;
    };
    let Some(page) = pdf.pages().get(page_index) else {
        return fallback;
    };
    page.initial_transform(true).to_kurbo().as_coeffs()
}

fn apply_mat(m: &[f64; 6], x: f64, y: f64) -> (f64, f64) {
    (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
}

fn invert_mat(m: &[f64; 6]) -> [f64; 6] {
    let det = m[0] * m[3] - m[1] * m[2];
    if det.abs() < 1e-12 {
        return [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
    }
    let inv = [
        m[3] / det,
        -m[1] / det,
        -m[2] / det,
        m[0] / det,
        (m[2] * m[5] - m[3] * m[4]) / det,
        (m[1] * m[4] - m[0] * m[5]) / det,
    ];
    inv
}

/// User-space axis-aligned rect [x, y, w, h] → display-space [x, y, w, h].
fn user_rect_to_display(m: &[f64; 6], r: [f32; 4]) -> [f32; 4] {
    let (x0, y0) = apply_mat(m, r[0] as f64, r[1] as f64);
    let (x1, y1) = apply_mat(m, (r[0] + r[2]) as f64, (r[1] + r[3]) as f64);
    let (minx, maxx) = (x0.min(x1), x0.max(x1));
    let (miny, maxy) = (y0.min(y1), y0.max(y1));
    [
        minx as f32,
        miny as f32,
        (maxx - minx) as f32,
        (maxy - miny) as f32,
    ]
}

fn annot_color_css(c: [f32; 3]) -> String {
    format!(
        "rgb({}, {}, {})",
        (c[0] * 255.0) as u8,
        (c[1] * 255.0) as u8,
        (c[2] * 255.0) as u8
    )
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
    let page_dims = RwSignal::new(Vec::<(f32, f32)>::new());
    let page_xforms = RwSignal::new(Vec::<[f64; 6]>::new());
    let page_spans = RwSignal::new(Vec::<Vec<TextSpan>>::new());
    let page_annots = RwSignal::new(Vec::<Vec<AnnotInfo>>::new());
    let mode = RwSignal::new(Mode::View);
    let undo_stack = RwSignal::new(Vec::<Vec<u8>>::new());
    let redo_stack = RwSignal::new(Vec::<Vec<u8>>::new());
    let query = RwSignal::new(String::new());
    let replacement = RwSignal::new(String::new());
    let hits = RwSignal::new(Vec::<SearchHit>::new());
    let hit_index = RwSignal::new(0usize);
    // drag state: (page_index, start_x, start_y, cur_x, cur_y) in display points
    let drag = RwSignal::new(Option::<(usize, f32, f32, f32, f32)>::None);
    let ink_trail = RwSignal::new(Vec::<(f32, f32)>::new());
    let ink_page = RwSignal::new(Option::<usize>::None);

    let push_undo = move || {
        let bytes = pdf_bytes.get_untracked();
        if bytes.is_empty() {
            return;
        }
        undo_stack.update(|s| {
            s.push(bytes);
            if s.len() > 50 {
                s.remove(0);
            }
        });
        redo_stack.set(Vec::new());
    };

    let undo = move || {
        let Some(prev) = undo_stack.try_update(|s| s.pop()).flatten() else {
            status.set("Nothing to undo.".to_string());
            return;
        };
        redo_stack.update(|s| s.push(pdf_bytes.get_untracked()));
        pdf_bytes.set(prev);
        hits.set(Vec::new());
        status.set("Undone.".to_string());
    };

    let redo = move || {
        let Some(next) = redo_stack.try_update(|s| s.pop()).flatten() else {
            status.set("Nothing to redo.".to_string());
            return;
        };
        undo_stack.update(|s| s.push(pdf_bytes.get_untracked()));
        pdf_bytes.set(next);
        hits.set(Vec::new());
        status.set("Redone.".to_string());
    };

    // recompute derived per-page data + re-render canvases when doc or zoom changes
    Effect::new(move || {
        let bytes = pdf_bytes.get();
        let z = zoom.get();
        if bytes.is_empty() {
            page_dims.set(vec![]);
            page_spans.set(vec![]);
            page_annots.set(vec![]);
            return;
        }
        let n = match pdf_edit_core::PdfDoc::load(&bytes) {
            Ok(d) => d.page_count(),
            Err(_) => 0,
        };
        page_count.set(n);
        page_dims.set(page_display_sizes(&bytes));
        page_xforms.set((0..n).map(|i| page_user_to_display(&bytes, i)).collect());
        page_spans.set(
            (0..n)
                .map(|i| pdf_edit_core::text::text_spans(&bytes, i).unwrap_or_default())
                .collect(),
        );
        page_annots.set(
            match pdf_edit_core::PdfDoc::load(&bytes) {
                Ok(d) => (1..=n as u32).map(|p| d.annotations(p)).collect(),
                Err(_) => vec![Vec::new(); n],
            },
        );
        request_animation_frame(move || {
            let Some(document) = web_sys::window().and_then(|w| w.document()) else {
                return;
            };
            for i in 0..n {
                if let Some(el) = document.get_element_by_id(&format!("page-canvas-{i}")) {
                    let canvas: web_sys::HtmlCanvasElement = el.unchecked_into();
                    if let Err(e) = render_page_to_canvas(&bytes, i, z, &canvas) {
                        web_sys::console::error_1(&format!("render p{i}: {e}").into());
                    }
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

    // undo/redo keyboard shortcuts
    let _kbd = window_event_listener(leptos::ev::keydown, move |ev| {
        if !(ev.ctrl_key() || ev.meta_key()) {
            return;
        }
        let key = ev.key();
        if key == "z" && !ev.shift_key() {
            ev.prevent_default();
            undo();
        } else if key == "y" || (key == "z" && ev.shift_key()) {
            ev.prevent_default();
            redo();
        }
    });

    let scroll_to_page = move |i: usize| {
        let Some(document) = web_sys::window().and_then(|w| w.document()) else {
            return;
        };
        if let Some(el) = document.get_element_by_id(&format!("page-wrap-{i}")) {
            el.scroll_into_view();
        }
    };

    let on_scroll = move |ev: leptos::ev::Event| {
        let Some(el) = ev.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()) else {
            return;
        };
        let n = page_count.get_untracked();
        if n == 0 {
            return;
        }
        let mid = el.scroll_top() as f64 + el.client_height() as f64 / 3.0;
        let mut current = 0usize;
        for i in 0..n {
            if let Some(child) = el.query_selector(&format!("#page-wrap-{i}")).ok().flatten() {
                let top = child
                    .dyn_ref::<web_sys::HtmlElement>()
                    .map(|h| h.offset_top() as f64)
                    .unwrap_or(0.0);
                if top <= mid {
                    current = i;
                }
            }
        }
        page.set(current);
    };

    let extract_from = RwSignal::new(1u32);
    let extract_to = RwSignal::new(1u32);

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
                            extract_to.set(doc.page_count() as u32);
                            page.set(0);
                            zoom.set(1.0);
                            undo_stack.set(Vec::new());
                            redo_stack.set(Vec::new());
                            hits.set(Vec::new());
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
                push_undo();
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
                            push_undo();
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

    // ---- find & replace ----

    let run_search = move || -> usize {
        let bytes = pdf_bytes.get_untracked();
        let q = query.get_untracked();
        if bytes.is_empty() || q.is_empty() {
            hits.set(Vec::new());
            return 0;
        }
        match pdf_edit_core::text::search(&bytes, &q) {
            Ok(h) => {
                let n = h.len();
                hits.set(h);
                hit_index.set(0);
                n
            }
            Err(e) => {
                status.set(format!("search failed: {e}"));
                hits.set(Vec::new());
                0
            }
        }
    };

    let on_find = move |_| {
        let n = run_search();
        if n == 0 {
            status.set("No matches.".to_string());
        } else {
            status.set(format!("{n} match(es)."));
            let p = hits.get_untracked().first().map(|h| h.page).unwrap_or(0);
            scroll_to_page(p);
        }
    };

    let step_hit = move |delta: i64| {
        let n = hits.get_untracked().len();
        if n == 0 {
            return;
        }
        hit_index.update(|i| {
            *i = ((*i as i64 + delta).rem_euclid(n as i64)) as usize;
        });
        let p = hits.get_untracked()[hit_index.get_untracked()].page;
        scroll_to_page(p);
    };

    let do_replace = move |all: bool| {
        let bytes = pdf_bytes.get_untracked();
        let current = hits.get_untracked();
        if bytes.is_empty() || current.is_empty() {
            status.set("Find text first.".to_string());
            return;
        }
        let repl = replacement.get_untracked();
        let result = if all {
            pdf_edit_core::PdfDoc::load(&bytes).and_then(|mut d| {
                let (ip, ov) =
                    d.find_and_replace(&bytes, &query.get_untracked(), &current, &repl)?;
                d.save().map(|out| (out, ip, ov))
            })
        } else {
            let one = current[hit_index.get_untracked()].clone();
            pdf_edit_core::PdfDoc::load(&bytes).and_then(|mut d| {
                let ov = d.overlay_replace(&bytes, std::slice::from_ref(&one), &repl)?;
                d.save().map(|out| (out, 0usize, ov))
            })
        };
        match result {
            Ok((out, ip, ov)) => {
                push_undo();
                pdf_bytes.set(out);
                status.set(format!("Replaced {} (in-place {ip}, overlay {ov}).", ip + ov));
                run_search();
            }
            Err(e) => status.set(format!("replace failed: {e}")),
        }
    };

    // ---- annotation interactions ----

    let identity = [1.0f64, 0.0, 0.0, 1.0, 0.0, 0.0];
    let user_mat = move |page_index: usize| -> [f64; 6] {
        invert_mat(&page_xforms.with(|x| x.get(page_index).copied().unwrap_or(identity)))
    };
    let fwd_mat = move |page_index: usize| -> [f64; 6] {
        page_xforms.with(|x| x.get(page_index).copied().unwrap_or(identity))
    };

    let overlay_point = move |ev: &web_sys::PointerEvent| -> (f32, f32) {
        let z = zoom.get_untracked().max(0.05);
        (
            ev.offset_x() as f32 / z,
            ev.offset_y() as f32 / z,
        )
    };

    let on_pointer_down = move |page_index: usize, ev: web_sys::PointerEvent| {
        let m = mode.get_untracked();
        if m == Mode::View {
            return;
        }
        ev.prevent_default();
        if let Some(t) = ev.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()) {
            let _ = t.set_pointer_capture(ev.pointer_id());
        }
        let (x, y) = overlay_point(&ev);
        match m {
            Mode::Ink => {
                ink_page.set(Some(page_index));
                ink_trail.set(vec![(x, y)]);
            }
            Mode::Note => {
                let window = web_sys::window().expect("window");
                if let Some(text) = window.prompt_with_message("Note text:").ok().flatten() {
                    if !text.is_empty() {
                        let m_user = user_mat(page_index);
                        let (ux, uy) = apply_mat(&m_user, x as f64, y as f64);
                        apply_edit(&|d| {
                            let _ = d.add_note(page_index as u32 + 1, ux as f32, uy as f32, &text);
                        });
                        status.set("Note added.".to_string());
                    }
                }
            }
            _ => drag.set(Some((page_index, x, y, x, y))),
        }
    };

    let on_pointer_move = move |ev: web_sys::PointerEvent| {
        match mode.get_untracked() {
            Mode::Ink => {
                if ink_page.get_untracked().is_some() {
                    let (x, y) = overlay_point(&ev);
                    ink_trail.update(|t| t.push((x, y)));
                }
            }
            Mode::View | Mode::Note => {}
            _ => {
                if let Some((p, x0, y0, _, _)) = drag.get_untracked() {
                    let (x, y) = overlay_point(&ev);
                    drag.set(Some((p, x0, y0, x, y)));
                }
            }
        }
    };

    let on_pointer_up = move |_ev: web_sys::PointerEvent| {
        match mode.get_untracked() {
            Mode::Ink => {
                let Some(page_index) = ink_page.get_untracked() else {
                    return;
                };
                ink_page.set(None);
                let trail = ink_trail.get_untracked();
                ink_trail.set(Vec::new());
                if trail.len() < 2 {
                    return;
                }
                let m_user = user_mat(page_index);
                let stroke: Vec<(f32, f32)> = trail
                    .iter()
                    .map(|&(x, y)| {
                        let (ux, uy) = apply_mat(&m_user, x as f64, y as f64);
                        (ux as f32, uy as f32)
                    })
                    .collect();
                apply_edit(&|d| {
                    let _ = d.add_ink(
                        page_index as u32 + 1,
                        std::slice::from_ref(&stroke),
                        [0.1, 0.2, 0.8],
                        2.0,
                    );
                });
                status.set("Ink added.".to_string());
            }
            Mode::View | Mode::Note => {}
            _ => {
                let Some((page_index, x0, y0, x1, y1)) = drag.get_untracked() else {
                    return;
                };
                drag.set(None);
                let (minx, maxx) = (x0.min(x1), x0.max(x1));
                let (miny, maxy) = (y0.min(y1), y0.max(y1));
                if maxx - minx < 2.0 || maxy - miny < 2.0 {
                    return;
                }
                let m_user = user_mat(page_index);
                let (ux0, uy0) = apply_mat(&m_user, minx as f64, miny as f64);
                let (ux1, uy1) = apply_mat(&m_user, maxx as f64, maxy as f64);
                let quad = [
                    ux0.min(ux1) as f32,
                    uy0.min(uy1) as f32,
                    (ux1 - ux0).abs() as f32,
                    (uy1 - uy0).abs() as f32,
                ];
                let (subtype, color) = match mode.get_untracked() {
                    Mode::Highlight => ("Highlight", [1.0, 1.0, 0.0]),
                    Mode::Underline => ("Underline", [0.2, 0.7, 0.2]),
                    _ => ("StrikeOut", [0.9, 0.2, 0.2]),
                };
                apply_edit(&|d| {
                    let _ = d.add_markup(page_index as u32 + 1, subtype, &[quad], color, "");
                });
                status.set(format!("{subtype} added."));
            }
        }
    };

    let delete_annot = move |page_index: usize, idx: usize| {
        let window = web_sys::window().expect("window");
        if !window.confirm_with_message("Delete this annotation?").unwrap_or(false) {
            return;
        }
        apply_edit(&|d| {
            let _ = d.delete_annot(page_index as u32 + 1, idx);
        });
        status.set("Annotation deleted.".to_string());
    };

    view! {
        <main style="font-family: system-ui; max-width: 1180px; margin: 1.5rem auto; padding: 0 1rem;">
            <h1 style="margin-bottom: 0.25rem;">"pdf-edit"</h1>
            <p style="margin-top: 0; color: #555;">"Files never leave your device."</p>
            <input type="file" accept="application/pdf" on:change=on_file />

            <div style="margin: 0.75rem 0; display: flex; gap: 0.4rem; align-items: center; flex-wrap: wrap;">
                {MODES.into_iter().map(|m| {
                    view! {
                        <button
                            style=move || format!(
                                "{}",
                                if mode.get() == m {
                                    "background: #2463eb; color: white; border: 1px solid #2463eb; border-radius: 4px; padding: 2px 8px;"
                                } else {
                                    "background: white; border: 1px solid #999; border-radius: 4px; padding: 2px 8px;"
                                }
                            )
                            on:click=move |_| mode.set(m)
                        >{m.label()}</button>
                    }
                }).collect_view()}
                <span style="border-left: 1px solid #ccc; padding-left: 0.4rem;">
                    <button title="Undo (Ctrl+Z)" disabled=move || undo_stack.with(|s| s.is_empty()) on:click=move |_| undo()>"⤺ Undo"</button>
                    <button title="Redo (Ctrl+Y)" disabled=move || redo_stack.with(|s| s.is_empty()) on:click=move |_| redo()>"⤻ Redo"</button>
                </span>
                <span style="border-left: 1px solid #ccc; padding-left: 0.4rem;">
                    <button on:click=move |_| set_zoom(|z| z / 1.25)>"−"</button>
                    <button on:click=move |_| set_zoom(|_| 1.0)>{move || format!("{}%", (zoom.get() * 100.0).round())}</button>
                    <button on:click=move |_| set_zoom(|z| z * 1.25)>"+"</button>
                </span>
                <button style="font-weight: 600;" on:click=on_save>"Save PDF"</button>
                <span>{move || status.get()}</span>
            </div>

            <div style="margin: 0.25rem 0 0.5rem; display: flex; gap: 0.5rem; align-items: center; flex-wrap: wrap; font-size: 14px;">
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

            <div style="margin: 0.25rem 0 0.75rem; display: flex; gap: 0.4rem; align-items: center; flex-wrap: wrap; font-size: 14px;">
                <input
                    type="text"
                    placeholder="Find text…"
                    style="padding: 2px 6px; border: 1px solid #999; border-radius: 4px;"
                    prop:value=move || query.get()
                    on:input=move |ev| query.set(event_target_value(&ev))
                    on:keydown=move |ev| { if ev.key() == "Enter" { on_find(()); } }
                />
                <button on:click=move |_| on_find(())>"Find"</button>
                <button title="Previous match" on:click=move |_| step_hit(-1)>"↑"</button>
                <button title="Next match" on:click=move |_| step_hit(1)>"↓"</button>
                <span>{move || {
                    let n = hits.get().len();
                    if n == 0 { String::new() } else { format!("{}/{}", hit_index.get() + 1, n) }
                }}</span>
                <input
                    type="text"
                    placeholder="Replace with…"
                    style="padding: 2px 6px; border: 1px solid #999; border-radius: 4px;"
                    prop:value=move || replacement.get()
                    on:input=move |ev| replacement.set(event_target_value(&ev))
                />
                <button on:click=move |_| do_replace(false)>"Replace"</button>
                <button on:click=move |_| do_replace(true)>"Replace all"</button>
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
                                        on:click=move |_| scroll_to_page(i)
                                    />
                                    <div style="display: flex; justify-content: space-between; align-items: center; font-size: 12px; margin-top: 2px;">
                                        <span>{num}</span>
                                        <span style="display: flex; gap: 2px;">
                                            <button title="Rotate 90°" on:click=move |_| apply_edit(&|d| { let _ = d.rotate_page_by(num as u32, 90); })>"↻"</button>
                                            <button title="Move up" on:click=move |_| { if num > 1 { apply_edit(&|d| { let _ = d.move_page(num as u32, num as u32 - 1); }); } }>"↑"</button>
                                            <button title="Move down" on:click=move |_| { if num < page_count.get() { apply_edit(&|d| { let _ = d.move_page(num as u32, num as u32 + 1); }); } }>"↓"</button>
                                            <button title="Delete page" on:click=move |_| { if page_count.get() > 1 { apply_edit(&|d| d.delete_page(num as u32)); } }>"✕"</button>
                                        </span>
                                    </div>
                                </div>
                            }
                        }).collect_view()
                    }}
                </div>

                <div
                    id="pages-scroll"
                    style="flex: 1; max-height: 80vh; overflow-y: auto; border: 1px solid #ccc; background: #f5f5f5; padding: 12px;"
                    on:scroll=on_scroll
                >
                    {move || {
                        let n = page_count.get();
                        let z = zoom.get();
                        let dims = page_dims.get();
                        (0..n).map(|i| {
                            let (w, h) = dims.get(i).copied().unwrap_or((612.0, 792.0));
                            let (cw, ch) = (w * z, h * z);
                            view! {
                                <div
                                    id=format!("page-wrap-{i}")
                                    style="margin: 0 auto 16px; width: fit-content; background: white; box-shadow: 0 1px 4px rgba(0,0,0,0.3);"
                                >
                                    <div style=format!("position: relative; width: {cw}px; height: {ch}px;")>
                                        <canvas
                                            id=format!("page-canvas-{i}")
                                            style="position: absolute; left: 0; top: 0; display: block;"
                                        ></canvas>
                                        <div style="position: absolute; left: 0; top: 0; width: 100%; height: 100%; pointer-events: none;">
                                            {move || {
                                                page_spans.with(|ps| {
                                                    ps.get(i).cloned().unwrap_or_default().into_iter().map(|s| {
                                                        view! {
                                                            <span style=format!(
                                                                "position: absolute; left: {}px; top: {}px; width: {}px; height: {}px; font-size: {}px; line-height: 1; font-family: sans-serif; color: transparent; overflow: hidden; white-space: pre; pointer-events: auto; user-select: text;",
                                                                s.x * z, s.y * z, s.w * z, s.h * z, s.h * z
                                                            )>{s.text}</span>
                                                        }
                                                    }).collect_view()
                                                })
                                            }}
                                        </div>
                                        {move || {
                                            hits.get().iter().enumerate().filter(|(_, h)| h.page == i).map(|(hi, h)| {
                                                let active = hi == hit_index.get();
                                                h.spans.iter().map(move |s| {
                                                    view! {
                                                        <div style=format!(
                                                            "position: absolute; left: {}px; top: {}px; width: {}px; height: {}px; pointer-events: none; {};",
                                                            s.x * z, s.y * z, s.w * z, s.h * z,
                                                            if active {
                                                                "background: rgba(255, 140, 0, 0.45)"
                                                            } else {
                                                                "background: rgba(255, 230, 0, 0.4)"
                                                            }
                                                        )></div>
                                                    }
                                                }).collect_view()
                                            }).collect_view()
                                        }}
                                        {move || {
                                            let m_fwd = fwd_mat(i);
                                            page_annots.with(|pa| {
                                                pa.get(i).cloned().unwrap_or_default().into_iter().enumerate().map(|(ai, a)| {
                                                    let deletable = mode.get() == Mode::View;
                                                    match a.subtype.as_str() {
                                                        "Ink" => {
                                                            let points: String = a.ink.iter().flatten().map(|&(x, y)| {
                                                                let (dx, dy) = apply_mat(&m_fwd, x as f64, y as f64);
                                                                format!("{},{}", dx as f32 * z, dy as f32 * z)
                                                            }).collect::<Vec<_>>().join(" ");
                                                            let color = annot_color_css(a.color);
                                                            view! {
                                                                <svg
                                                                    style="position: absolute; left: 0; top: 0; width: 100%; height: 100%; pointer-events: none;"
                                                                    viewBox=format!("0 0 {cw} {ch}")
                                                                >
                                                                    <polyline points=points fill="none" stroke=color stroke-width="2" stroke-linecap="round" stroke-linejoin="round" />
                                                                </svg>
                                                            }.into_any()
                                                        }
                                                        "Text" => {
                                                            let r = user_rect_to_display(&m_fwd, a.rect);
                                                            view! {
                                                                <div
                                                                    title=a.contents.clone()
                                                                    style=format!(
                                                                        "position: absolute; left: {}px; top: {}px; width: 16px; height: 16px; background: #ffd54a; border: 1px solid #b8860b; border-radius: 3px; cursor: pointer;",
                                                                        r[0] * z, r[1] * z
                                                                    )
                                                                    on:click=move |_| { if deletable { delete_annot(i, ai); } }
                                                                ></div>
                                                            }.into_any()
                                                        }
                                                        _ => {
                                                            let color = annot_color_css(a.color);
                                                            a.quads.iter().map(|&q| {
                                                                let r = user_rect_to_display(&m_fwd, q);
                                                                let style = match a.subtype.as_str() {
                                                                    "Highlight" => format!(
                                                                        "position: absolute; left: {}px; top: {}px; width: {}px; height: {}px; background: {}; opacity: 0.35; cursor: pointer;",
                                                                        r[0] * z, r[1] * z, r[2] * z, r[3] * z, color
                                                                    ),
                                                                    "Underline" => format!(
                                                                        "position: absolute; left: {}px; top: {}px; width: {}px; height: 2px; background: {}; cursor: pointer;",
                                                                        r[0] * z, (r[1] + r[3]) * z - 2.0, r[2] * z, color
                                                                    ),
                                                                    _ => format!(
                                                                        "position: absolute; left: {}px; top: {}px; width: {}px; height: 2px; background: {}; cursor: pointer;",
                                                                        r[0] * z, (r[1] + r[3] * 0.5) * z, r[2] * z, color
                                                                    ),
                                                                };
                                                                view! {
                                                                    <div
                                                                        style=style
                                                                        on:click=move |_| { if deletable { delete_annot(i, ai); } }
                                                                    ></div>
                                                                }.into_any()
                                                            }).collect_view().into_any()
                                                        }
                                                    }
                                                }).collect_view()
                                            })
                                        }}
                                        {move || {
                                            drag.get().and_then(|(p, x0, y0, x1, y1)| {
                                                if p != i { return None; }
                                                let (minx, maxx) = (x0.min(x1) * z, x0.max(x1) * z);
                                                let (miny, maxy) = (y0.min(y1) * z, y0.max(y1) * z);
                                                Some(view! {
                                                    <div style=format!(
                                                        "position: absolute; left: {minx}px; top: {miny}px; width: {}px; height: {}px; border: 1px dashed #2463eb; background: rgba(36,99,235,0.12); pointer-events: none;",
                                                        maxx - minx, maxy - miny
                                                    )></div>
                                                })
                                            })
                                        }}
                                        {move || {
                                            if ink_page.get() == Some(i) {
                                                let trail = ink_trail.get();
                                                let points: String = trail.iter().map(|&(x, y)| format!("{},{}", x * z, y * z)).collect::<Vec<_>>().join(" ");
                                                Some(view! {
                                                    <svg style="position: absolute; left: 0; top: 0; width: 100%; height: 100%; pointer-events: none;" viewBox=format!("0 0 {cw} {ch}")>
                                                        <polyline points=points fill="none" stroke="rgb(26,51,204)" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" />
                                                    </svg>
                                                })
                                            } else {
                                                None
                                            }
                                        }}
                                        <div
                                            style=move || format!(
                                                "position: absolute; left: 0; top: 0; width: 100%; height: 100%; {};",
                                                if mode.get() == Mode::View {
                                                    "pointer-events: none;"
                                                } else {
                                                    "pointer-events: auto; cursor: crosshair; touch-action: none;"
                                                }
                                            )
                                            on:pointerdown=move |ev| on_pointer_down(i, ev)
                                            on:pointermove=on_pointer_move
                                            on:pointerup=on_pointer_up
                                        ></div>
                                    </div>
                                </div>
                            }
                        }).collect_view()
                    }}
                </div>
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
