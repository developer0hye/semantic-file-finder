# CLAUDE.md

## Gemini API 개발 가이드라인

- Gemini API 사용 시 반드시 Google 공식 문서를 참고할 것
  - API Reference: https://ai.google.dev/gemini-api/docs
  - Supported file types: https://ai.google.dev/gemini-api/docs/vision (이미지), https://ai.google.dev/gemini-api/docs/document-processing (문서)
  - Embedding API: https://ai.google.dev/gemini-api/docs/embeddings
- 기본 모델: `gemini-3-flash-preview` (임베딩: `gemini-embedding-001`, 1536차원)
- 모델 ID, 지원 포맷, 파라미터 등은 변경될 수 있으므로 구현 전 최신 공식 문서에서 확인 필수
- LLM이 생성한 정보(지원 포맷, API 스펙 등)를 그대로 신뢰하지 말고 공식 문서로 검증할 것

## 개발 환경 원칙

- 패키지 설치가 필요한 작업은 무조건 Docker 기반으로 개발하고 테스트할 것
- 로컬 환경에 직접 pip install, npm install 등을 하지 말고 Docker 컨테이너 안에서 실행
- 실험, 빌드, 테스트 모두 Dockerfile 또는 docker-compose로 재현 가능하게 구성
- `.env` 파일은 프로젝트 루트에 하나만 두고, 각 docker-compose.yml에서 `env_file: ../../.env`로 참조

## 실험(experiments) 구조

- `experiments/` 아래에 기능별 하위 폴더를 만들어 실험을 관리할 것
- 각 실험 폴더는 독립적으로 실행 가능해야 함 (자체 Dockerfile, docker-compose.yml, requirements.txt 포함)
- 구조 예시:
  ```
  experiments/
  ├── image-understanding/    # 이미지 포함 문서의 Gemini 이해도 비교 실험
  ├── embedding-quality/      # 임베딩 품질 평가 실험 (예시)
  └── ...
  ```
