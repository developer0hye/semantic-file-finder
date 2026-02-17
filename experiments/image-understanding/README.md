# Image Understanding Experiment

DOCX/PPTX 내 임베디드 이미지를 Gemini에 보내는 3가지 방식을 비교하여, 문서 이해도 차이를 측정하는 실험.

## 배경

MarkItDown(Microsoft)은 DOCX/PPTX를 Markdown으로 변환하지만, 임베디드 이미지를 제대로 처리하지 못한다.
이미지에만 존재하는 정보(차트 수치, 다이어그램 등)를 Gemini가 이해하려면 이미지를 별도로 추출하여 함께 전송해야 한다.

## 실험 설계

### 3가지 케이스

| Case | 방식 | 설명 |
|------|------|------|
| **A** | 텍스트만 | MarkItDown Markdown만 전송. 이미지 없음. |
| **B** | 텍스트 + 이미지 (순서 없이) | Markdown 원문 + ZIP에서 추출한 이미지를 나열. |
| **C** | 텍스트 + 이미지 (마커 매칭) | Markdown 내 이미지 참조를 `[IMAGE_N]` 마커로 치환하고, 각 이미지를 `[IMAGE_N]:` 라벨과 함께 순서대로 매칭하여 전송. |

### 테스트 파일

`create_test_files.py`로 생성한 DOCX/PPTX:
- 텍스트에는 일반적인 서술만 포함 (구체적 수치 없음)
- 차트 이미지에만 핵심 수치 포함:
  - 분기별 매출: Q1=120억, Q2=145억, Q3=198억, Q4=176억
  - 시장 점유율: A사 42%, B사 28%, C사 18%, 기타 12%
  - 2019~2024 매출/이익 추이 (2020년 적자 포함)

## 실행 방법

```bash
# 프로젝트 루트의 .env에 GEMINI_API_KEY 설정 필요

# 1. 테스트 파일 생성
docker compose run --rm experiment python create_test_files.py

# 2. DOCX 실험
docker compose run --rm experiment python test_image_understanding.py test_files/test_report.docx

# 3. PPTX 실험
docker compose run --rm experiment python test_image_understanding.py test_files/test_presentation.pptx
```

## 실험 결과 (2026-02-17, gemini-3-flash-preview)

### DOCX (test_report.docx)

#### Summary 비교

| Case | Summary | 구체적 수치 |
|------|---------|------------|
| **A (텍스트만)** | "record-breaking revenue results in the third quarter...strong growth across all segments" | X — 수치 없음 |
| **B (이미지 나열)** | "quarterly revenue reaching a record peak of 198 (100M KRW) in Q3...Company A maintains a dominant 42% market share" | O — Q3=198억, 42% |
| **C (마커 매칭)** | "quarterly revenue peaking at 198 (100M KRW) in Q3...leads the market with a 42% share" | O — Q3=198억, 42% |

#### Image Descriptions 비교

| Case | 이미지 1 (매출 차트) | 이미지 2 (점유율) | 이미지 3 (추이) |
|------|---------------------|------------------|----------------|
| **A** | "A chart displaying quarterly revenue" (추측) | "A pie chart illustrating market share" (추측) | "A historical growth chart" (추측) |
| **B** | "Q1=120, Q2=145, Q3=198, Q4=176" (정확) | "A=42%, B=28%, C=18%, Others=12%" (정확) | "Revenue dip in 2020, climb to ~200 by 2024" (정확) |
| **C** | "Q1=120, Q2=145, Q3=198, Q4=176" (정확) | "A=42%, B=28%, C=18%, Others=12%" (정확) | "Revenue dip in 2020, upward trend to 2024" (정확) |

#### 정량 비교

```
이미지 설명 개수:  A=3  B=3  C=3
Summary 길이:     A=304  B=364  C=363
Keywords 개수:    A=8    B=8    C=8
```

## 결론

1. **A vs B/C: 차이 극명** — 이미지를 함께 전송하면 차트 속 수치(매출, 점유율 등)를 정확히 추출. 텍스트만으로는 불가능.
2. **B vs C: 이번 실험에서는 결과 유사** — 이미지 3개, 각각 뚜렷하게 다른 유형이라 Gemini가 문맥만으로도 매칭 가능.
3. **C 방식 채택** — 이미지가 많아지거나 유사한 차트가 여러 개인 경우를 대비하여, `[IMAGE_N]` 마커 매칭 방식을 표준으로 사용.

## 구현에 대한 시사점

- DOCX/PPTX 처리 시 MarkItDown 텍스트 추출 + ZIP 이미지 추출을 병행해야 함
- OOXML Relationship ID(rId) 파싱으로 이미지 등장 순서 보장
- Gemini 전송 시 `[IMAGE_N]:` 라벨 + 이미지 바이너리를 순서대로 매칭
- XLSX는 이미지가 거의 없으므로 텍스트만 처리해도 무방
