# PTMD — PDF To Markdown

Rust CLI that converts text-based PDFs to GitHub-Flavored Markdown using [anydoc](https://crates.io/crates/anydoc) (Firecrawl, pure Rust via `pdf-inspector`), with OCR fallback for scanned/image-only pages via [faster-paddle](https://pypi.org/project/faster-paddle/) (small model).

## Pipeline

```
PDF
  │
  ├─► anydoc::to_markdown()     → full text → Markdown (if all text-based)
  │
  └─► ConvertError::NeedsOcr
        │
        ├─► text pages:  pdfseparate → anydoc (per-page)
        └─► image pages: pdftoppm → faster-paddle OCR (small)
              │
              └─► combine all pages → document.md
```

## Usage

```bash
# Install
cargo build --release

# Convert a PDF
./target/release/ptmd --pdf input.pdf --outdir output/

# With custom DPI for OCR rendering
./target/release/ptmd --pdf input.pdf --outdir output/ --dpi 300
```

Output:
- `document.md` — combined Markdown with `<!-- PAGE N -->` markers
- `manifest.json` — source hash (SHA256), page count, native/OCR breakdown

## Dependencies

- **anydoc** 0.2 (Rust crate) — text-based PDF extraction via pdf-inspector
- **faster-paddle** (Python) — OCR for image-only pages, small model
- **pdftoppm** (poppler) — render PDF pages to PNG for OCR
- **pdfseparate** (poppler) — split single pages for per-page anydoc extraction

## Install Python OCR dependency

```bash
pip install faster-paddle
```

## License

MIT
