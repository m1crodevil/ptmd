#!/usr/bin/env python3
"""OCR a single PDF page with faster_paddle (small model).

Args: pdf_path page_num [dpi]
Prints OCR text to stdout.
"""
import sys, subprocess, tempfile, os

pdf_path = sys.argv[1]
page_num = int(sys.argv[2])
dpi = int(sys.argv[3]) if len(sys.argv) > 3 else 150

with tempfile.TemporaryDirectory() as tmpdir:
    prefix = os.path.join(tmpdir, "page")
    subprocess.run(
        ["pdftoppm", "-f", str(page_num), "-l", str(page_num),
         "-r", str(dpi), "-png", pdf_path, prefix],
        check=True, capture_output=True
    )
    png_files = [f for f in os.listdir(tmpdir) if f.endswith(".png")]
    if not png_files:
        print("")
        sys.exit(0)
    png_path = os.path.join(tmpdir, png_files[0])
    with open(png_path, "rb") as f:
        image_bytes = f.read()

from faster_paddle import OcrEngine
engine = OcrEngine(model_size="small")
result = engine.ocr(image_bytes)
print(result["text"])
