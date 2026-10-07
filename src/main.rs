// PTMD — PDF To Markdown
// Detection + text extraction via pdf_inspector (Firecrawl, pure Rust, lopdf).
// OCR fallback via faster-paddle (small model) for image-only pages.
//
// Pipeline:
//   1. extract_pages_markdown_mem() → per-page markdown + needs_ocr flag
//   2. Text pages → use extracted markdown directly
//   3. OCR pages → faster-paddle (small model)
//   4. Combine → document.md
//
// Usage: ptmd --pdf <path> --outdir <dir> [--dpi 150]

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;
use sha2::Digest;

const DEFAULT_DPI: u32 = 150;

#[derive(Serialize)]
struct Manifest {
    source_hash: String,
    page_count: usize,
    native_pages: usize,
    ocr_pages: usize,
    dpi: u32,
    converted_at: String,
    engine: String,
    pdf_type: String,
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut pdf_path = String::new();
    let mut outdir = String::new();
    let mut dpi = DEFAULT_DPI;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--pdf" => { pdf_path = args.get(i + 1).cloned().unwrap_or_default(); i += 2; }
            "--outdir" => { outdir = args.get(i + 1).cloned().unwrap_or_default(); i += 2; }
            "--dpi" => { dpi = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(DEFAULT_DPI); i += 2; }
            _ => { eprintln!("Unknown arg: {}", args[i]); i += 1; }
        }
    }

    if pdf_path.is_empty() || outdir.is_empty() {
        eprintln!("Usage: ptmd --pdf <path> --outdir <dir> [--dpi 150]");
        std::process::exit(1);
    }

    let pdf = Path::new(&pdf_path);
    if !pdf.exists() {
        eprintln!("PDF not found: {}", pdf_path);
        std::process::exit(1);
    }

    let outdir_path = PathBuf::from(&outdir);
    fs::create_dir_all(&outdir_path).unwrap();

    let source_hash = hash_file(pdf).expect("hash failed");
    let bytes = fs::read(pdf).expect("read pdf");

    // 1. Classify + extract per-page markdown in one pass
    let result = match pdf_inspector::extract_pages_markdown_mem(&bytes, None) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("pdf_inspector failed: {:?}", e);
            std::process::exit(2);
        }
    };

    let page_count = result.pages.len();
    let pdf_type = format!("{:?}", pdf_inspector::classify_pdf_mem(&bytes).map(|c| c.pdf_type).unwrap_or(pdf_inspector::PdfType::Mixed));
    let ocr_count = result.pages.iter().filter(|p| p.needs_ocr).count();

    eprintln!("pdf_inspector: type={} pages={} ocr={}", pdf_type, page_count, ocr_count);

    // 2. Build document.md: native text from pdf_inspector, OCR from faster-paddle
    let script_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/ocr_page.py");
    let mut document_md = String::new();
    let mut native_pages = 0;

    for page in &result.pages {
        let page_num = page.page as usize + 1; // 0-indexed → 1-indexed

        if page.needs_ocr {
            eprintln!("  page {} → OCR ({})", page_num, page.ocr_reason.as_deref().unwrap_or("unknown"));
            let ocr_text = ocr_page(&pdf_path, page_num, dpi, &script_path);
            document_md.push_str(&format!("<!-- PAGE {} -->\n\n{}\n\n", page_num, ocr_text.trim()));
        } else {
            native_pages += 1;
            document_md.push_str(&format!("<!-- PAGE {} -->\n\n{}\n\n", page_num, page.markdown.trim()));
        }
    }

    // 3. Write output
    let manifest = Manifest {
        source_hash,
        page_count,
        native_pages,
        ocr_pages: ocr_count,
        dpi,
        converted_at: chrono_now(),
        engine: if ocr_count > 0 { "pdf-inspector+faster-paddle".into() } else { "pdf-inspector".into() },
        pdf_type,
    };

    fs::write(outdir_path.join("document.md"), &document_md).expect("write document.md");
    fs::write(outdir_path.join("manifest.json"),
        serde_json::to_string_pretty(&manifest).unwrap()).expect("write manifest.json");

    eprintln!("PTMD bundle ready at: {}", outdir_path.display());
    eprintln!("  type: {} | pages: {} (native: {}, ocr: {})", manifest.pdf_type, page_count, native_pages, ocr_count);
}

fn ocr_page(pdf_path: &str, page_num: usize, dpi: u32, script_path: &Path) -> String {
    match Command::new("python3")
        .arg(script_path)
        .arg(pdf_path)
        .arg(page_num.to_string())
        .arg(dpi.to_string())
        .output()
    {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).to_string(),
        Ok(out) => {
            eprintln!("  OCR failed page {}: {}", page_num, String::from_utf8_lossy(&out.stderr));
            String::new()
        }
        Err(e) => {
            eprintln!("  OCR subprocess failed page {}: {}", page_num, e);
            String::new()
        }
    }
}

fn hash_file(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let mut file = fs::File::open(path)?;
    let mut hasher = sha2::Sha256::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 { break; }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn chrono_now() -> String {
    match Command::new("date").arg("+%Y-%m-%dT%H:%M:%S%:z").output() {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        _ => "unknown".to_string(),
    }
}
