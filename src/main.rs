// PTMD — PDF To Markdown
// Text extraction via anydoc (Firecrawl, pure Rust, pdf-inspector for PDF).
// OCR fallback via faster-paddle (small model) for image-only pages.
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
    let page_count = get_page_count(pdf).unwrap_or(0);
    eprintln!("Pages: {}", page_count);

    // Try anydoc on the full PDF first
    match anydoc::to_markdown(&pdf_path) {
        Ok(md) => {
            // All text — no OCR needed
            eprintln!("anydoc: OK (all text, no OCR needed)");
            let md_path = outdir_path.join("document.md");
            fs::write(&md_path, &md).expect("write document.md");

            let manifest = Manifest {
                source_hash,
                page_count,
                native_pages: page_count,
                ocr_pages: 0,
                dpi,
                converted_at: chrono_now(),
                engine: "anydoc".to_string(),
            };
            write_manifest(&outdir_path, &manifest);
            eprintln!("PTMD bundle ready at: {}", outdir);
            eprintln!("  pages: {} (native: {}, ocr: 0)", page_count, page_count);
            return;
        }
        Err(anydoc::ConvertError::NeedsOcr { pages, .. }) => {
            eprintln!("anydoc: NeedsOcr — {} pages need OCR: {:?}", pages.len(), pages);
            let pages_usize: Vec<usize> = pages.iter().map(|&p| p as usize).collect();
            let combined = handle_mixed_pdf(&pdf_path, page_count, &pages_usize, dpi);
            let native_pages = page_count - pages.len();

            let md_path = outdir_path.join("document.md");
            fs::write(&md_path, &combined).expect("write document.md");

            let manifest = Manifest {
                source_hash,
                page_count,
                native_pages,
                ocr_pages: pages.len(),
                dpi,
                converted_at: chrono_now(),
                engine: "anydoc+faster-paddle".to_string(),
            };
            write_manifest(&outdir_path, &manifest);
            eprintln!("PTMD bundle ready at: {}", outdir);
            eprintln!("  pages: {} (native: {}, ocr: {})", page_count, native_pages, pages.len());
            return;
        }
        Err(e) => {
            eprintln!("anydoc failed: {:?}", e);
            std::process::exit(2);
        }
    }
}

/// Handle mixed PDF: text pages via anydoc, OCR pages via faster-paddle.
fn handle_mixed_pdf(pdf_path: &str, page_count: usize, ocr_pages: &[usize], dpi: u32) -> String {
    let ocr_set: std::collections::HashSet<usize> = ocr_pages.iter().copied().collect();
    let script_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/ocr_page.py");
    let tmpdir = std::env::temp_dir().join(format!("ptmd_{}", std::process::id()));
    fs::create_dir_all(&tmpdir).unwrap();

    let mut page_texts: BTreeMap<usize, String> = BTreeMap::new();

    // Extract each page as a single-page PDF, then run anydoc on text pages
    // and faster-paddle on OCR pages
    for page_num in 1..=page_count {
        if ocr_set.contains(&page_num) {
            // OCR page: render to image + faster-paddle
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
            // Text page: extract as single-page PDF → anydoc
            let prefix = tmpdir.join("page");
            let _ = Command::new("pdfseparate")
                .arg("-f").arg(page_num.to_string())
                .arg("-l").arg(page_num.to_string())
                .arg(pdf_path)
                .arg(prefix.to_str().unwrap())
                .output();

            // pdfseparate creates page-N.pdf (or page-N-N.pdf) — find it
            let expected_name = format!("page-{}.pdf", page_num);
            let single = tmpdir.join(&expected_name);
            // Also try page-N-N.pdf format
            let alt_name = format!("page-{}-{}.pdf", page_num, page_num);
            let alt = tmpdir.join(&alt_name);
            let single = if single.exists() { single } else if alt.exists() { alt } else { single.clone() };
            if single.exists() {
                match anydoc::to_markdown(single.to_str().unwrap()) {
                    Ok(md) => { page_texts.insert(page_num, md); }
                    Err(e) => {
                        eprintln!("  anydoc failed page {}: {:?}, trying pdftotext", page_num, e);
                        // Fallback to pdftotext for this page
                        let r = Command::new("pdftotext")
                            .arg("-f").arg(page_num.to_string())
                            .arg("-l").arg(page_num.to_string())
                            .arg(pdf_path)
                            .arg("-")
                            .output();
                        if r.is_ok() {
                            page_texts.insert(page_num, String::from_utf8_lossy(&r.unwrap().stdout).to_string());
                        } else {
                            page_texts.insert(page_num, String::new());
                        }
                    }
                }
                let _ = fs::remove_file(&single);
            } else {
                // pdfseparate might have failed, try pdftotext
                let r = Command::new("pdftotext")
                    .arg("-f").arg(page_num.to_string())
                    .arg("-l").arg(page_num.to_string())
                    .arg(pdf_path)
                    .arg("-")
                    .output();
                if r.is_ok() {
                    page_texts.insert(page_num, String::from_utf8_lossy(&r.unwrap().stdout).to_string());
                } else {
                    page_texts.insert(page_num, String::new());
                }
            }
        }
    }

    // Cleanup
    let _ = fs::remove_dir_all(&tmpdir);

    // Combine with page markers
    let mut document_md = String::new();
    for page_num in 1..=page_count {
        let text = page_texts.get(&page_num).cloned().unwrap_or_default();
        document_md.push_str(&format!("<!-- PAGE {} -->\n\n{}\n\n", page_num, text.trim()));
    }
    document_md
}

fn write_manifest(outdir: &Path, manifest: &Manifest) {
    let path = outdir.join("manifest.json");
    fs::write(&path, serde_json::to_string_pretty(manifest).unwrap())
        .expect("write manifest.json");
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

fn get_page_count(pdf: &Path) -> Option<usize> {
    let out = Command::new("pdfinfo").arg(pdf).output().ok()?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    for line in stdout.lines() {
        if line.starts_with("Pages:") {
            return line.split(':').nth(1).and_then(|s| s.trim().parse().ok());
        }
    }
    None
}

fn chrono_now() -> String {
    let out = Command::new("date").arg("+%Y-%m-%dT%H:%M:%S%:z").output();
    match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        _ => "unknown".to_string(),
    }
}
