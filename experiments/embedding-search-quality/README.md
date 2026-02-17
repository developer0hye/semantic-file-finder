# Embedding Search Quality Experiment

## 목적

Gemini Embedding API(`gemini-embedding-001`, 1536차원)를 사용하여 문서를 인덱싱하고,
자연어 쿼리로 검색했을 때 cosine similarity 기반 랭킹이 올바른 문서를 찾는지 검증.

## 실험 설계

### 테스트 문서 (10개)

서로 다른 도메인의 한국어 DOCX 문서:

| # | 파일명 | 주제 |
|---|--------|------|
| 1 | `finance_q3_revenue_report.docx` | 3분기 매출 실적 보고서 |
| 2 | `hr_backend_developer_jd.docx` | 백엔드 개발자 채용 공고 |
| 3 | `project_ai_chatbot_proposal.docx` | AI 챗봇 도입 제안서 |
| 4 | `legal_service_contract.docx` | 소프트웨어 개발 계약서 |
| 5 | `meeting_product_launch_20241015.docx` | 신제품 출시 회의록 |
| 6 | `tech_api_documentation.docx` | OAuth 인증 API 기술 문서 |
| 7 | `marketing_2025_strategy.docx` | 2025년 마케팅 전략 |
| 8 | `research_battery_technology.docx` | 리튬-황 배터리 연구 보고서 |
| 9 | `trip_report_tokyo_2024.docx` | 도쿄 출장 보고서 |
| 10 | `training_git_basics.docx` | Git 기초 교육 자료 |

### 인덱싱 파이프라인

1. MarkItDown으로 DOCX → Markdown 변환
2. Gemini Flash로 문서 분석 (summary + keywords 추출)
3. `"{summary} {keywords}"` 텍스트를 `gemini-embedding-001`로 임베딩 (1536차원)
4. `task_type=RETRIEVAL_DOCUMENT` 사용

### 검색 쿼리 (15개)

**직접 표현 (10개)**: 문서 내용과 직접 관련된 키워드 사용

| 쿼리 | 정답 문서 |
|-------|-----------|
| 3분기 매출 실적 | finance_q3_revenue_report.docx |
| 백엔드 개발자 채용 | hr_backend_developer_jd.docx |
| AI 챗봇 도입 비용 | project_ai_chatbot_proposal.docx |
| 소프트웨어 개발 계약 조건 | legal_service_contract.docx |
| 신제품 출시 일정 | meeting_product_launch_20241015.docx |
| OAuth 로그인 API 스펙 | tech_api_documentation.docx |
| 내년 마케팅 예산 계획 | marketing_2025_strategy.docx |
| 리튬 배터리 연구 동향 | research_battery_technology.docx |
| 도쿄 출장 경비 | trip_report_tokyo_2024.docx |
| Git 브랜치 전략 | training_git_basics.docx |

**간접 표현 (5개)**: 키워드가 정확히 매칭되지 않는 자연어 표현

| 쿼리 | 정답 문서 |
|-------|-----------|
| 김 부장이 일본 다녀온 보고서 | trip_report_tokyo_2024.docx |
| 클라우드 매출이 많이 올랐다는 문서 | finance_q3_revenue_report.docx |
| 파이썬 개발자 뽑는 공고 | hr_backend_developer_jd.docx |
| 전기차 배터리 관련 자료 | research_battery_technology.docx |
| 앱 출시 전 해야 할 일 | meeting_product_launch_20241015.docx |

## 결과

### 설정

- Analysis Model: `gemini-3-flash-preview`
- Embedding Model: `gemini-embedding-001`
- Dimensions: 1536
- Similarity: Cosine similarity
- Query task_type: `RETRIEVAL_QUERY`
- Document task_type: `RETRIEVAL_DOCUMENT`

### 정확도

| 지표 | 결과 |
|------|------|
| **Top-1 정확도** | **15/15 (100%)** |
| **Top-3 정확도** | **15/15 (100%)** |
| 직접 표현 Top-1 | 10/10 (100%) |
| 간접 표현 Top-1 | 5/5 (100%) |

### 유사도 점수 분포

정답 문서(1위)와 2위 문서 간 점수 차이:

| 쿼리 | 1위 점수 | 2위 점수 | 차이 |
|-------|---------|---------|------|
| 3분기 매출 실적 | 0.7798 | 0.6054 | 0.1744 |
| 백엔드 개발자 채용 | 0.7885 | 0.6099 | 0.1786 |
| AI 챗봇 도입 비용 | 0.7530 | 0.6298 | 0.1232 |
| 소프트웨어 개발 계약 조건 | 0.7300 | 0.5986 | 0.1314 |
| 신제품 출시 일정 | 0.7270 | 0.6052 | 0.1218 |
| OAuth 로그인 API 스펙 | 0.7419 | 0.5707 | 0.1712 |
| 내년 마케팅 예산 계획 | 0.7889 | 0.6668 | 0.1221 |
| 리튬 배터리 연구 동향 | 0.7015 | 0.5516 | 0.1499 |
| 도쿄 출장 경비 | 0.7602 | 0.5891 | 0.1711 |
| Git 브랜치 전략 | 0.7472 | 0.5887 | 0.1585 |
| 김 부장이 일본 다녀온 보고서 | 0.7162 | 0.6027 | 0.1135 |
| 클라우드 매출이 많이 올랐다는 문서 | 0.7546 | 0.6089 | 0.1457 |
| 파이썬 개발자 뽑는 공고 | 0.7371 | 0.6358 | 0.1013 |
| 전기차 배터리 관련 자료 | 0.6922 | 0.5819 | 0.1103 |
| 앱 출시 전 해야 할 일 | 0.7063 | 0.6078 | 0.0985 |

- 1위 평균 점수: **0.742**
- 1위-2위 평균 차이: **0.131** (충분한 마진)

## 결론

1. **Gemini embedding + cosine similarity 검색이 프로젝트에 충분히 사용 가능한 수준**
2. 직접 키워드뿐 아니라 간접 표현(자연어)도 100% 정확도 달성
3. 1위와 2위 간 점수 차이가 평균 0.131로, 오탐 가능성이 낮음
4. Summary + Keywords를 합쳐서 임베딩하는 전략이 효과적

## 실행 방법

```bash
# 프로젝트 루트의 .env에 GEMINI_API_KEY 설정 필요

# Docker로 실행
cd experiments/embedding-search-quality

# 1. 테스트 문서 생성
docker compose run --rm experiment python create_test_docs.py

# 2. 인덱싱 + 검색 테스트
docker compose run --rm experiment python test_embedding_search.py
```

## 상세 결과

`results/search_quality_gemini-embedding-001_1536d.json` 참고.
