// PTMD — PDF To Markdown
// Detection via pdf_inspector (Firecrawl, pure Rust) — classifies TextBased/Scanned/ImageBased/Mixed
// with per-page OCR reasons. Text extraction via anydoc. OCR fallback via faster-paddle (small).
//
// For mixed PDFs (common in Indonesian Tbk financial reports):
//   1. classify_pdf_mem() → PdfType + pages_needing_ocr (~10-50ms)
//   2. Text pages → anydoc::to_markdown() on single-page extract
//   3. OCR pages → faster-paddle (small model)
//   4. Combine → document.md
//
// Usage: ptmd --pdf <path> --outdir <dir> [--dpi 150]

use std::collections::BTreeMap;
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
    ocr_reasons: BTreeMap<usize, Vec<String>>,
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

    // 1. Classify via pdf_inspector (~10-50ms)
    let classification = match pdf_inspector::classify_pdf_mem(&bytes) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("pdf_inspector classify failed: {:?}, trying anydoc directly", e);
            match anydoc::to_markdown(&pdf_path) {
                Ok(md) => {
                    write_output(&outdir_path, &md, &Manifest {
                        source_hash, page_count: 0, native_pages: 0, ocr_pages: 0,
                        dpi, converted_at: chrono_now(), engine: "anydoc".into(),
                        pdf_type: "TextBased".into(), ocr_reasons: BTreeMap::new(),
                    });
                    return;
                }
                Err(e) => { eprintln!("anydoc also failed: {:?}", e); std::process::exit(2); }
            }
        }
    };

    let pdf_type = format!("{:?}", classification.pdf_type);
    let page_count: usize = classification.page_count as usize;
    eprintln!("pdf_inspector: type={} pages={}", pdf_type, page_count);

    let ocr_pages: Vec<usize> = classification.pages_needing_ocr
        .iter().map(|&p| p as usize + 1).collect(); // 0-indexed → 1-indexed

    eprintln!("pdf_inspector: type={} pages={} ocr_pages={:?} confidence={:.2}",
        pdf_type, page_count, ocr_pages, classification.confidence);

    match classification.pdf_type {
        pdf_inspector::PdfType::TextBased => {
            eprintln!("  → TextBased: extracting via anydoc");
            let md = anydoc::to_markdown(&pdf_path)
                .unwrap_or_else(|e| {
                    eprintln!("anydoc failed: {:?}, trying pdf_inspector extract", e);
                    extract_via_pdf_inspector(&bytes, page_count as u32)
                });
            write_output(&outdir_path, &md, &Manifest {
                source_hash, page_count, native_pages: page_count, ocr_pages: 0,
                dpi, converted_at: chrono_now(), engine: "anydoc".into(),
                pdf_type, ocr_reasons: BTreeMap::new(),
            });
        }
        _ => {
            eprintln!("  → {:?}: {} pages need OCR", classification.pdf_type, ocr_pages.len());
            for &p in &ocr_pages {
                eprintln!("    page {} needs OCR", p);
            }
            let md = handle_mixed(&pdf_path, page_count, &ocr_pages, dpi);
            let native = page_count - ocr_pages.len();
            write_output(&outdir_path, &md, &Manifest {
                source_hash, page_count, native_pages: native, ocr_pages: ocr_pages.len(),
                dpi, converted_at: chrono_now(), engine: "anydoc+faster-paddle".into(),
                pdf_type, ocr_reasons: BTreeMap::new(),
            });
        }
    }
}

/// Handle mixed/scanned PDF: text pages via anydoc, OCR pages via faster-paddle.
fn handle_mixed(pdf_path: &str, page_count: usize, ocr_pages: &[usize], dpi: u32) -> String {
    let ocr_set: std::collections::HashSet<usize> = ocr_pages.iter().copied().collect();
    let script_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/ocr_page.py");
    let tmpdir = std::env::temp_dir().join(format!("ptmd_{}", std::process::id()));
    fs::create_dir_all(&tmpdir).unwrap();

    let mut page_texts: BTreeMap<usize, String> = BTreeMap::new();

    for page_num in 1..=page_count {
        if ocr_set.contains(&page_num) {
            let result = Command::new("python3")
                .arg(&script_path)
                .arg(pdf_path)
                .arg(page_num.to_string())
                .arg(dpi.to_string())
                .output();
            match result {
                Ok(out) if out.status.success() => {
                    page_texts.insert(page_num, String::from_utf8_lossy(&out.stdout).to_string());
                }
                Ok(out) => {
                    eprintln!("  OCR failed page {}: {}", page_num, String::from_utf8_lossy(&out.stderr));
                    page_texts.insert(page_num, String::new());
                }
                Err(e) => {
                    eprintln!("  OCR subprocess failed page {}: {}", page_num, e);
                    page_texts.insert(page_num, String::new());
                }
            }
        } else {
            // Text page: extract single-page PDF → anydoc
            let prefix = tmpdir.join("page");
            let _ = Command::new("pdfseparate")
                .arg("-f").arg(page_num.to_string())
                .arg("-l").arg(page_num.to_string())
                .arg(pdf_path)
                .arg(prefix.to_str().unwrap())
                .output();

            let expected_name = format!("page-{}.pdf", page_num);
            let single = tmpdir.join(&expected_name);
            let alt_name = format!("page-{}-{}.pdf", page_num, page_num);
            let alt = tmpdir.join(&alt_name);
            let single = if single.exists() { single } else if alt.exists() { alt } else { single.clone() };

            if single.exists() {
                match anydoc::to_markdown(single.to_str().unwrap()) {
                    Ok(md) => { page_texts.insert(page_num, md); }
                    Err(e) => {
                        eprintln!("  anydoc failed page {}: {:?}, trying pdftotext", page_num, e);
                        let r = Command::new("pdftotext")
                            .arg("-f").arg(page_num.to_string())
                            .arg("-l").arg(page_num.to_string())
                            .arg(pdf_path).arg("-").output();
                        if let Ok(out) = r {
                            page_texts.insert(page_num, String::from_utf8_lossy(&out.stdout).to_string());
                        } else {
                            page_texts.insert(page_num, String::new());
                        }
                    }
                }
                let _ = fs::remove_file(&single);
            } else {
                let r = Command::new("pdftotext")
                    .arg("-f").arg(page_num.to_string())
                    .arg("-l").arg(page_num.to_string())
                    .arg(pdf_path).arg("-").output();
                if let Ok(out) = r {
                    page_texts.insert(page_num, String::from_utf8_lossy(&out.stdout).to_string());
                } else {
                    page_texts.insert(page_num, String::new());
                }
            }
        }
    }

    let _ = fs::remove_dir_all(&tmpdir);

    let mut document_md = String::new();
    for page_num in 1..=page_count {
        let text = page_texts.get(&page_num).cloned().unwrap_or_default();
        document_md.push_str(&format!("<!-- PAGE {} -->\n\n{}\n\n", page_num, text.trim()));
    }
    document_md
}

fn extract_via_pdf_inspector(bytes: &[u8], page_count: u32) -> String {
    match pdf_inspector::process_pdf_mem(bytes) {
        Ok(result) => result.markdown.unwrap_or_default(),
        Err(_) => {
            let mut md = String::new();
            for p in 1..=page_count {
                let r = std::process::Command::new("pdftotext")
                    .arg("-f").arg(p.to_string())
                    .arg("-l").arg(p.to_string())
                    .arg("-")
                    .output();
                if let Ok(out) = r {
                    md.push_str(&format!("<!-- PAGE {} -->\n\n{}\n\n", p,
                        String::from_utf8_lossy(&out.stdout).trim()));
                }
            }
            md
        }
    }
}

fn write_output(outdir: &Path, md: &str, manifest: &Manifest) {
    fs::write(outdir.join("document.md"), md).expect("write document.md");
    fs::write(outdir.join("manifest.json"),
        serde_json::to_string_pretty(manifest).unwrap()).expect("write manifest.json");
    eprintln!("PTMD bundle ready at: {}", outdir.display());
    eprintln!("  type: {} | pages: {} (native: {}, ocr: {})",
        manifest.pdf_type, manifest.page_count, manifest.native_pages, manifest.ocr_pages);
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
    let out = Command::new("date").arg("+%Y-%m-%dT%H:%M:%S%:z").output();
    match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        _ => "unknown".to_string(),
    }
}
