"""
임베딩 검색 품질 실험

10개의 서로 다른 주제의 문서를 인덱싱하고,
자연어 쿼리로 검색하여 cosine similarity 랭킹이 올바른지 검증.

사용법:
  # 1. 문서 생성
  python create_test_docs.py

  # 2. 인덱싱 + 검색 테스트
  python test_embedding_search.py
"""

import os
import sys
import json
import time
import re
import zipfile
from pathlib import Path

import numpy as np
from google import genai
from markitdown import MarkItDown
from lxml import etree

# ---------------------------------------------------------------------------
# Config
# ---------------------------------------------------------------------------

TEST_FILES_DIR = Path(__file__).parent / "test_files"
RESULTS_DIR = Path(__file__).parent / "results"

ANALYSIS_PROMPT = """Analyze the following document and respond in JSON.

1. summary: Summarize the document content in 2-3 sentences. Be specific — include numbers, names, and key data points.
2. keywords: Array of 5-10 key terms (in the document's language)

Response format:
{
  "summary": "...",
  "keywords": ["...", "..."]
}"""

# ---------------------------------------------------------------------------
# Document Processing (reused from image-understanding experiment)
# ---------------------------------------------------------------------------

DOCX_NS = {
    "a": "http://schemas.openxmlformats.org/drawingml/2006/main",
    "r": "http://schemas.openxmlformats.org/officeDocument/2006/relationships",
    "rel": "http://schemas.openxmlformats.org/package/2006/relationships",
}


def extract_images_docx(file_path: str) -> list[tuple[str, bytes]]:
    images = []
    with zipfile.ZipFile(file_path) as z:
        rels_path = "word/_rels/document.xml.rels"
        if rels_path not in z.namelist():
            return images
        rels_tree = etree.parse(z.open(rels_path))
        rid_to_path = {}
        for rel in rels_tree.iter("{%s}Relationship" % DOCX_NS["rel"]):
            if "image" in rel.get("Type", ""):
                rid_to_path[rel.get("Id")] = rel.get("Target")
        doc_tree = etree.parse(z.open("word/document.xml"))
        for blip in doc_tree.iter("{%s}blip" % DOCX_NS["a"]):
            rid = blip.get("{%s}embed" % DOCX_NS["r"])
            if rid and rid in rid_to_path:
                target = rid_to_path[rid]
                img_path = target if target.startswith("word/") else f"word/{target}"
                try:
                    images.append((os.path.basename(img_path), z.read(img_path)))
                except KeyError:
                    pass
    return images


def get_markdown(file_path: str) -> str:
    md = MarkItDown()
    return md.convert(file_path).text_content


def add_image_markers(markdown_text: str) -> str:
    counter = [0]
    def replace_img(match):
        counter[0] += 1
        return f"[IMAGE_{counter[0]}]"
    return re.sub(r"!\[.*?\]\(.*?\)", replace_img, markdown_text)


# ---------------------------------------------------------------------------
# Gemini API
# ---------------------------------------------------------------------------

def analyze_document(client: genai.Client, model: str, text: str, images: list) -> dict:
    """문서 분석: summary + keywords 추출."""
    if images:
        marked_text = add_image_markers(text)
        parts = [
            f"Document content (images marked as [IMAGE_N]):\n\n{marked_text}",
        ]
        for i, (filename, img_bytes) in enumerate(images, 1):
            ext = Path(filename).suffix.lower()
            mime = {"png": "image/png", "jpg": "image/jpeg", "jpeg": "image/jpeg"}.get(
                ext.lstrip("."), "image/png"
            )
            parts.append(f"[IMAGE_{i}]:")
            parts.append(genai.types.Part.from_bytes(data=img_bytes, mime_type=mime))
        parts.append(ANALYSIS_PROMPT)
    else:
        parts = [f"Document content:\n\n{text}", ANALYSIS_PROMPT]

    response = client.models.generate_content(
        model=model,
        contents=parts,
        config=genai.types.GenerateContentConfig(
            response_mime_type="application/json",
        ),
    )
    return json.loads(response.text)


def get_embedding(
    client: genai.Client,
    model: str,
    text: str,
    task_type: str,
    dimensions: int,
) -> list[float]:
    """텍스트의 임베딩 벡터를 생성."""
    response = client.models.embed_content(
        model=model,
        contents=text,
        config=genai.types.EmbedContentConfig(
            task_type=task_type,
            output_dimensionality=dimensions,
        ),
    )
    return response.embeddings[0].values


def cosine_similarity(a: list[float], b: list[float]) -> float:
    a, b = np.array(a), np.array(b)
    norm_a, norm_b = np.linalg.norm(a), np.linalg.norm(b)
    if norm_a == 0 or norm_b == 0:
        return 0.0
    return float(np.dot(a, b) / (norm_a * norm_b))


# ---------------------------------------------------------------------------
# Indexing
# ---------------------------------------------------------------------------

def index_documents(client: genai.Client, model: str, embed_model: str, dimensions: int):
    """모든 테스트 문서를 인덱싱."""
    files = sorted(TEST_FILES_DIR.glob("*.docx"))
    if not files:
        print("No test files found. Run create_test_docs.py first.")
        sys.exit(1)

    index = []
    for i, file_path in enumerate(files):
        print(f"\n[{i+1}/{len(files)}] Indexing: {file_path.name}")

        # 텍스트 + 이미지 추출
        text = get_markdown(str(file_path))
        images = extract_images_docx(str(file_path))
        print(f"  Text: {len(text)} chars, Images: {len(images)}")

        # Gemini 분석
        analysis = analyze_document(client, model, text, images)
        summary = analysis.get("summary", "")
        keywords = analysis.get("keywords", [])
        print(f"  Summary: {summary[:80]}...")
        print(f"  Keywords: {', '.join(keywords[:5])}...")

        # 임베딩 생성
        embed_text = f"{summary} {' '.join(keywords)}"
        embedding = get_embedding(
            client, embed_model, embed_text, "RETRIEVAL_DOCUMENT", dimensions
        )
        print(f"  Embedding: {len(embedding)} dims")

        index.append({
            "file": file_path.name,
            "summary": summary,
            "keywords": keywords,
            "embedding": embedding,
        })

        # Rate limit 대비
        time.sleep(1)

    return index


# ---------------------------------------------------------------------------
# Search
# ---------------------------------------------------------------------------

# 검색 쿼리 + 정답 파일 (expected top-1)
QUERIES = [
    {
        "query": "3분기 매출 실적",
        "expected": "finance_q3_revenue_report.docx",
        "description": "재무 보고서 검색",
    },
    {
        "query": "백엔드 개발자 채용",
        "expected": "hr_backend_developer_jd.docx",
        "description": "채용 공고 검색",
    },
    {
        "query": "AI 챗봇 도입 비용",
        "expected": "project_ai_chatbot_proposal.docx",
        "description": "프로젝트 제안서 검색",
    },
    {
        "query": "소프트웨어 개발 계약 조건",
        "expected": "legal_service_contract.docx",
        "description": "계약서 검색",
    },
    {
        "query": "신제품 출시 일정",
        "expected": "meeting_product_launch_20241015.docx",
        "description": "회의록 검색",
    },
    {
        "query": "OAuth 로그인 API 스펙",
        "expected": "tech_api_documentation.docx",
        "description": "기술 문서 검색",
    },
    {
        "query": "내년 마케팅 예산 계획",
        "expected": "marketing_2025_strategy.docx",
        "description": "마케팅 전략 검색",
    },
    {
        "query": "리튬 배터리 연구 동향",
        "expected": "research_battery_technology.docx",
        "description": "연구 보고서 검색",
    },
    {
        "query": "도쿄 출장 경비",
        "expected": "trip_report_tokyo_2024.docx",
        "description": "출장 보고서 검색",
    },
    {
        "query": "Git 브랜치 전략",
        "expected": "training_git_basics.docx",
        "description": "교육 자료 검색",
    },
    # 간접적 표현 (키워드가 정확히 매칭되지 않는 경우)
    {
        "query": "김 부장이 일본 다녀온 보고서",
        "expected": "trip_report_tokyo_2024.docx",
        "description": "간접 표현 - 출장 보고서",
    },
    {
        "query": "클라우드 매출이 많이 올랐다는 문서",
        "expected": "finance_q3_revenue_report.docx",
        "description": "간접 표현 - 재무 보고서",
    },
    {
        "query": "파이썬 개발자 뽑는 공고",
        "expected": "hr_backend_developer_jd.docx",
        "description": "간접 표현 - 채용 공고",
    },
    {
        "query": "전기차 배터리 관련 자료",
        "expected": "research_battery_technology.docx",
        "description": "간접 표현 - 연구 보고서",
    },
    {
        "query": "앱 출시 전 해야 할 일",
        "expected": "meeting_product_launch_20241015.docx",
        "description": "간접 표현 - 회의록",
    },
]


def search(
    client: genai.Client,
    embed_model: str,
    dimensions: int,
    index: list,
    query: str,
) -> list[dict]:
    """쿼리로 검색하여 유사도 순으로 결과 반환."""
    query_embedding = get_embedding(
        client, embed_model, query, "RETRIEVAL_QUERY", dimensions
    )
    results = []
    for doc in index:
        score = cosine_similarity(query_embedding, doc["embedding"])
        results.append({
            "file": doc["file"],
            "score": score,
            "summary": doc["summary"],
        })
    results.sort(key=lambda x: x["score"], reverse=True)
    return results


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def main():
    api_key = os.environ.get("GEMINI_API_KEY")
    if not api_key:
        print("Error: GEMINI_API_KEY 환경변수를 설정해주세요")
        sys.exit(1)

    model = os.environ.get("GEMINI_MODEL", "gemini-3-flash-preview")
    embed_model = os.environ.get("GEMINI_EMBEDDING_MODEL", "gemini-embedding-001")
    dimensions = int(os.environ.get("GEMINI_EMBEDDING_DIMENSIONS", "1536"))

    print(f"Model: {model}")
    print(f"Embedding: {embed_model} ({dimensions} dims)")

    client = genai.Client(api_key=api_key)

    # Step 1: 인덱싱
    print("\n" + "=" * 60)
    print("  Step 1: Document Indexing")
    print("=" * 60)
    index = index_documents(client, model, embed_model, dimensions)

    # Step 2: 검색 테스트
    print("\n" + "=" * 60)
    print("  Step 2: Search Quality Test")
    print("=" * 60)

    total = len(QUERIES)
    top1_correct = 0
    top3_correct = 0
    all_results = []

    for i, q in enumerate(QUERIES):
        print(f"\n--- Query {i+1}/{total}: \"{q['query']}\" ({q['description']}) ---")

        results = search(client, embed_model, dimensions, index, q["query"])
        top1 = results[0]["file"]
        top3_files = [r["file"] for r in results[:3]]

        is_top1 = top1 == q["expected"]
        is_top3 = q["expected"] in top3_files

        if is_top1:
            top1_correct += 1
        if is_top3:
            top3_correct += 1

        status = "TOP1" if is_top1 else ("TOP3" if is_top3 else "MISS")
        print(f"  Expected: {q['expected']}")
        print(f"  Result:   {status}")
        for j, r in enumerate(results[:5]):
            marker = " <--" if r["file"] == q["expected"] else ""
            print(f"    {j+1}. {r['file']}  (score: {r['score']:.4f}){marker}")

        all_results.append({
            "query": q["query"],
            "description": q["description"],
            "expected": q["expected"],
            "top1": top1,
            "top1_correct": is_top1,
            "top3_correct": is_top3,
            "ranking": [{"file": r["file"], "score": r["score"]} for r in results[:5]],
        })

        time.sleep(0.5)

    # Summary
    print("\n" + "=" * 60)
    print("  Results Summary")
    print("=" * 60)
    print(f"  Total queries:   {total}")
    print(f"  Top-1 accuracy:  {top1_correct}/{total} ({top1_correct/total*100:.0f}%)")
    print(f"  Top-3 accuracy:  {top3_correct}/{total} ({top3_correct/total*100:.0f}%)")

    # 직접 표현 vs 간접 표현 구분
    direct = [r for r in all_results if "간접" not in r["description"]]
    indirect = [r for r in all_results if "간접" in r["description"]]

    if direct:
        d_top1 = sum(1 for r in direct if r["top1_correct"])
        print(f"\n  Direct queries:   Top-1 {d_top1}/{len(direct)} ({d_top1/len(direct)*100:.0f}%)")
    if indirect:
        i_top1 = sum(1 for r in indirect if r["top1_correct"])
        print(f"  Indirect queries: Top-1 {i_top1}/{len(indirect)} ({i_top1/len(indirect)*100:.0f}%)")

    # 결과 저장
    RESULTS_DIR.mkdir(parents=True, exist_ok=True)
    out_path = RESULTS_DIR / f"search_quality_{embed_model}_{dimensions}d.json"
    with open(out_path, "w", encoding="utf-8") as f:
        json.dump({
            "config": {
                "model": model,
                "embed_model": embed_model,
                "dimensions": dimensions,
            },
            "metrics": {
                "total": total,
                "top1_accuracy": top1_correct / total,
                "top3_accuracy": top3_correct / total,
            },
            "results": all_results,
        }, f, ensure_ascii=False, indent=2)
    print(f"\n  Results saved: {out_path}")


if __name__ == "__main__":
    main()
