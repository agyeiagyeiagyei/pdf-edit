# pdf-edit

Client-side PDF editor for the browser. Rust + WebAssembly, no server — files never leave the device.

**Status: early spike.** See below for the roadmap.

## Features (planned)

1. **Viewer + page organization** — zoom, thumbnails, rotate / delete / reorder pages, merge files, extract / split, save
2. **Annotations** — highlight, underline, strikeout, freehand ink, sticky notes, written as real PDF annotations
3. **Forms + signatures** — AcroForm filling, optional flatten, drawn-signature stamps, metadata editing
4. **Find & replace** — scoped text replacement (in-place when the embedded font allows, overlay fallback otherwise)

Out of scope: print production, OCR, redaction, freeform text/layout editing.

## Architecture

- `core/` — pure-Rust PDF manipulation library ([lopdf](https://crates.io/crates/lopdf)), unit-tested natively
- `app/` — [Leptos](https://leptos.dev) + [Trunk](https://trunkrs.dev) WASM frontend; pages rendered to canvas with [hayro](https://crates.io/crates/hayro)
- GitHub Actions builds and deploys the app to GitHub Pages on push to `main`

## Develop

```sh
rustup target add wasm32-unknown-unknown
cargo install trunk
cargo test -p pdf-edit-core   # native tests
cd app && trunk serve         # dev server
```

## License

Apache-2.0
