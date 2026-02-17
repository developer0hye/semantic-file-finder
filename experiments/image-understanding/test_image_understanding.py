"""
Gemini 문서 이해도 비교 실험

3가지 방식으로 동일 문서를 Gemini에 보내고 결과를 비교:
  A. 텍스트만 (MarkItDown Markdown)
  B. 텍스트 + 이미지 (순서 없이 나열)
  C. 텍스트 + 이미지 (IMAGE_N 마커 매칭)

사용법:
  export GEMINI_API_KEY="your-api-key"
  python test_image_understanding.py sample.docx
  python test_image_understanding.py sample.pptx
"""

import sys
import os
import re
import json
import zipfile
import time
from pathlib import Path

from google import genai
from markitdown import MarkItDown
from lxml import etree


# ---------------------------------------------------------------------------
# 1. 문서에서 텍스트 + 이미지 추출
# ---------------------------------------------------------------------------

# DOCX XML namespaces
DOCX_NS = {
    "a": "http://schemas.openxmlformats.org/drawingml/2006/main",
    "r": "http://schemas.openxmlformats.org/officeDocument/2006/relationships",
    "rel": "http://schemas.openxmlformats.org/package/2006/relationships",
}

# PPTX XML namespaces
PPTX_NS = {
    "a": "http://schemas.openxmlformats.org/drawingml/2006/main",
    "r": "http://schemas.openxmlformats.org/officeDocument/2006/relationships",
    "rel": "http://schemas.openxmlformats.org/package/2006/relationships",
    "p": "http://schemas.openxmlformats.org/presentationml/2006/main",
}


def extract_images_docx(file_path: str) -> list[tuple[str, bytes]]:
    """DOCX에서 등장 순서대로 이미지를 추출한다."""
    images = []

    with zipfile.ZipFile(file_path) as z:
        # rels 파일에서 rId → 이미지 경로 매핑
        rels_path = "word/_rels/document.xml.rels"
        if rels_path not in z.namelist():
            return images

        rels_tree = etree.parse(z.open(rels_path))
        rid_to_path = {}
        for rel in rels_tree.iter("{%s}Relationship" % DOCX_NS["rel"]):
            rel_type = rel.get("Type", "")
            if "image" in rel_type:
                rid_to_path[rel.get("Id")] = rel.get("Target")

        # document.xml에서 이미지 참조를 등장 순서대로 수집
        doc_tree = etree.parse(z.open("word/document.xml"))
        for blip in doc_tree.iter("{%s}blip" % DOCX_NS["a"]):
            rid = blip.get("{%s}embed" % DOCX_NS["r"])
            if rid and rid in rid_to_path:
                target = rid_to_path[rid]
                img_path = target if target.startswith("word/") else f"word/{target}"
                try:
                    img_bytes = z.read(img_path)
                    images.append((os.path.basename(img_path), img_bytes))
                except KeyError:
                    pass

    return images


def extract_images_pptx(file_path: str) -> list[tuple[str, bytes]]:
    """PPTX에서 슬라이드 순서대로 이미지를 추출한다."""
    images = []

    with zipfile.ZipFile(file_path) as z:
        # 슬라이드 파일 목록을 번호순으로 정렬
        slide_files = sorted(
            [n for n in z.namelist() if re.match(r"ppt/slides/slide\d+\.xml", n)],
            key=lambda x: int(re.search(r"slide(\d+)", x).group(1)),
        )

        for slide_file in slide_files:
            # 각 슬라이드의 rels 파일
            slide_name = os.path.basename(slide_file)
            rels_path = f"ppt/slides/_rels/{slide_name}.rels"
            if rels_path not in z.namelist():
                continue

            rels_tree = etree.parse(z.open(rels_path))
            rid_to_path = {}
            for rel in rels_tree.iter("{%s}Relationship" % PPTX_NS["rel"]):
                rel_type = rel.get("Type", "")
                if "image" in rel_type:
                    rid_to_path[rel.get("Id")] = rel.get("Target")

            # 슬라이드 XML에서 이미지 참조 순서대로 수집
            slide_tree = etree.parse(z.open(slide_file))
            for blip in slide_tree.iter("{%s}blip" % PPTX_NS["a"]):
                rid = blip.get("{%s}embed" % PPTX_NS["r"])
                if rid and rid in rid_to_path:
                    target = rid_to_path[rid]
                    if target.startswith("../"):
                        img_path = f"ppt/{target[3:]}"
                    elif target.startswith("ppt/"):
                        img_path = target
                    else:
                        img_path = f"ppt/media/{target}"
                    try:
                        img_bytes = z.read(img_path)
                        images.append((os.path.basename(img_path), img_bytes))
                    except KeyError:
                        pass

    return images


def extract_images(file_path: str) -> list[tuple[str, bytes]]:
    """파일 확장자에 따라 이미지를 추출한다."""
    ext = Path(file_path).suffix.lower()
    if ext == ".docx":
        return extract_images_docx(file_path)
    elif ext == ".pptx":
        return extract_images_pptx(file_path)
    else:
        return []


def get_markdown(file_path: str) -> str:
    """MarkItDown으로 텍스트를 Markdown으로 변환한다."""
    md = MarkItDown()
    result = md.convert(file_path)
    return result.text_content


def add_image_markers(markdown_text: str) -> str:
    """Markdown 내 이미지 참조를 [IMAGE_N] 마커로 치환한다."""
    counter = [0]

    def replace_img(match):
        counter[0] += 1
        return f"[IMAGE_{counter[0]}]"

    return re.sub(r"!\[.*?\]\(.*?\)", replace_img, markdown_text)


# ---------------------------------------------------------------------------
# 2. Gemini API 호출
# ---------------------------------------------------------------------------

ANALYSIS_PROMPT = """Analyze the following document and respond in JSON.

1. summary: Summarize the document content in 2-3 sentences. Be specific — include numbers, names, and key data points if present.
2. keywords: Array of 5-10 key terms
3. image_descriptions: For each image/chart/diagram you can see, describe its content in detail. If no images, return empty array.

Response format:
{
  "summary": "...",
  "keywords": ["...", "..."],
  "image_descriptions": ["description of image 1", "description of image 2"]
}"""


def mime_type_from_filename(filename: str) -> str:
    ext = Path(filename).suffix.lower()
    return {
        ".png": "image/png",
        ".jpg": "image/jpeg",
        ".jpeg": "image/jpeg",
        ".gif": "image/gif",
        ".bmp": "image/bmp",
        ".tiff": "image/tiff",
        ".webp": "image/webp",
        ".emf": "image/emf",
        ".wmf": "image/wmf",
    }.get(ext, "image/png")


def call_gemini(client: genai.Client, model: str, parts: list) -> dict:
    """Gemini generateContent를 호출하고 JSON 결과를 반환한다."""
    response = client.models.generate_content(
        model=model,
        contents=parts,
        config=genai.types.GenerateContentConfig(
            response_mime_type="application/json",
        ),
    )
    return json.loads(response.text)


def test_case_a(client: genai.Client, model: str, markdown_text: str) -> dict:
    """케이스 A: 텍스트만 전송"""
    parts = [
        f"Document content:\n\n{markdown_text}",
        ANALYSIS_PROMPT,
    ]
    return call_gemini(client, model, parts)


def test_case_b(
    client: genai.Client,
    model: str,
    markdown_text: str,
    images: list[tuple[str, bytes]],
) -> dict:
    """케이스 B: 텍스트 + 이미지 (순서 없이 나열)"""
    parts = [f"Document content:\n\n{markdown_text}"]
    for filename, img_bytes in images:
        mime = mime_type_from_filename(filename)
        parts.append(genai.types.Part.from_bytes(data=img_bytes, mime_type=mime))
    parts.append(ANALYSIS_PROMPT)
    return call_gemini(client, model, parts)


def test_case_c(
    client: genai.Client,
    model: str,
    markdown_text: str,
    images: list[tuple[str, bytes]],
) -> dict:
    """케이스 C: 텍스트 + 이미지 (IMAGE_N 마커 매칭)"""
    marked_text = add_image_markers(markdown_text)

    parts = [
        (
            "Document content (images are marked as [IMAGE_N], "
            "corresponding to the attached images in order):\n\n"
            f"{marked_text}"
        ),
    ]
    for i, (filename, img_bytes) in enumerate(images, 1):
        mime = mime_type_from_filename(filename)
        parts.append(f"[IMAGE_{i}]:")
        parts.append(genai.types.Part.from_bytes(data=img_bytes, mime_type=mime))
    parts.append(ANALYSIS_PROMPT)
    return call_gemini(client, model, parts)


# ---------------------------------------------------------------------------
# 3. 결과 출력
# ---------------------------------------------------------------------------

def print_result(label: str, result: dict):
    border = "=" * 60
    print(f"\n{border}")
    print(f"  {label}")
    print(border)
    print(f"\n[Summary]\n{result.get('summary', 'N/A')}")
    print(f"\n[Keywords]\n{', '.join(result.get('keywords', []))}")
    descriptions = result.get("image_descriptions", [])
    if descriptions:
        print(f"\n[Image Descriptions]")
        for i, desc in enumerate(descriptions, 1):
            print(f"  {i}. {desc}")
    else:
        print("\n[Image Descriptions]\n  (none)")
    print()


def save_results(file_path: str, model: str, results: dict):
    """결과를 JSON 파일로 저장한다."""
    results_dir = Path(__file__).parent / "results"
    results_dir.mkdir(parents=True, exist_ok=True)

    stem = Path(file_path).stem
    out_path = results_dir / f"{stem}_{model.replace('/', '_')}.json"
    with open(out_path, "w", encoding="utf-8") as f:
        json.dump(results, f, ensure_ascii=False, indent=2)
    print(f"Results saved: {out_path}")


def main():
    if len(sys.argv) < 2:
        print("Usage: python test_image_understanding.py <file.docx|file.pptx>")
        sys.exit(1)

    file_path = sys.argv[1]
    if not os.path.exists(file_path):
        print(f"File not found: {file_path}")
        sys.exit(1)

    api_key = os.environ.get("GEMINI_API_KEY")
    if not api_key:
        print("Error: GEMINI_API_KEY 환경변수를 설정해주세요")
        print("  export GEMINI_API_KEY='your-api-key'")
        sys.exit(1)

    model = os.environ.get("GEMINI_MODEL", "gemini-3-flash-preview")

    print(f"File:  {file_path}")
    print(f"Model: {model}")

    # 텍스트 추출
    print("\n[1/2] MarkItDown으로 텍스트 추출 중...")
    markdown_text = get_markdown(file_path)
    print(f"  → 텍스트 길이: {len(markdown_text)} chars")

    # 이미지 추출
    print("[2/2] 이미지 추출 중...")
    images = extract_images(file_path)
    print(f"  → 이미지 {len(images)}개 추출")
    for i, (name, data) in enumerate(images, 1):
        print(f"     {i}. {name} ({len(data):,} bytes)")

    if not images:
        print("\n⚠ 이미지가 없는 문서입니다. 케이스 A만 실행합니다.")

    client = genai.Client(api_key=api_key)
    all_results = {"file": file_path, "model": model, "image_count": len(images)}

    # 케이스 A: 텍스트만
    print("\n--- 케이스 A: 텍스트만 전송 ---")
    result_a = test_case_a(client, model, markdown_text)
    print_result("Case A: Text Only", result_a)
    all_results["case_a"] = result_a

    if images:
        # API rate limit 대비 잠시 대기
        time.sleep(2)

        # 케이스 B: 텍스트 + 이미지 (순서 없이)
        print("--- 케이스 B: 텍스트 + 이미지 (순서 없이) ---")
        result_b = test_case_b(client, model, markdown_text, images)
        print_result("Case B: Text + Images (unordered)", result_b)
        all_results["case_b"] = result_b

        time.sleep(2)

        # 케이스 C: 텍스트 + 이미지 (마커 매칭)
        print("--- 케이스 C: 텍스트 + 이미지 (마커 매칭) ---")
        result_c = test_case_c(client, model, markdown_text, images)
        print_result("Case C: Text + Images (matched markers)", result_c)
        all_results["case_c"] = result_c

        # 비교 요약
        print("=" * 60)
        print("  비교 요약")
        print("=" * 60)
        desc_a = len(result_a.get("image_descriptions", []))
        desc_b = len(result_b.get("image_descriptions", []))
        desc_c = len(result_c.get("image_descriptions", []))
        print(f"  이미지 설명 개수:  A={desc_a}  B={desc_b}  C={desc_c}")
        print(f"  Summary 길이:     A={len(result_a.get('summary',''))}  "
              f"B={len(result_b.get('summary',''))}  "
              f"C={len(result_c.get('summary',''))}")
        print(f"  Keywords 개수:    A={len(result_a.get('keywords',[]))}  "
              f"B={len(result_b.get('keywords',[]))}  "
              f"C={len(result_c.get('keywords',[]))}")
        print()

    save_results(file_path, model, all_results)


if __name__ == "__main__":
    main()
