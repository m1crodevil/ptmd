# PTMD — PDF To Markdown

Rust CLI that converts PDFs to Markdown using [pdf-inspector](https://crates.io/crates/pdf-inspector) (Firecrawl, pure Rust, lopdf), with OCR fallback for scanned/image-only pages via [faster-paddle](https://pypi.org/project/faster-paddle/) (small model).

## Pipeline

```
PDF
  │
  ├─► pdf_inspector::extract_pages_markdown_mem()
  │     → per-page markdown + needs_ocr flag (~10-50ms classification)
  │
  ├─► text pages:  use extracted markdown directly
  ├─► image pages: pdftoppm → faster-paddle OCR (small model)
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
- `manifest.json` — source hash (SHA256), page count, native/OCR breakdown, pdf_type

## Dependencies

- **pdf-inspector** 1.x (Rust crate) — PDF classification, per-page text extraction, Markdown conversion
- **faster-paddle** (Python) — OCR for image-only pages, small model
- **pdftoppm** (poppler) — render PDF pages to PNG for OCR

## Install Python OCR dependency

```bash
pip install faster-paddle
```

## License

MIT
