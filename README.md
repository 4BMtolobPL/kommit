# Kommit

**Kommit**은 AI를 사용하여 Git 변경 사항(`diff`)을 분석하고, [Conventional Commits](https://www.conventionalcommits.org/) 규격에 맞는 커밋
메시지를 자동으로 생성해주는 CLI 도구입니다.

## 주요 기능

- 🔍 **Diff 분석**: 스테이징된 변경 사항 또는 현재 작업 트리의 변경 사항을 분석합니다.
- 📝 **Conventional Commits**: `feat`, `fix`, `refactor` 등 표준화된 커밋 타입을 자동으로 분류합니다.
- 🌐 **다국어 지원**: 한국어와 영어 메시지 생성을 지원합니다.
- 🤖 **다양한 AI 프로바이더**: [Ollama](https://ollama.com/) 및 [LM Studio](https://lmstudio.ai/)와 같은 로컬 LLM을 지원합니다.

## 설치 방법

Rust 툴체인이 설치되어 있어야 합니다.

```bash
# 저장소 클론
git clone https://github.com/4BMtolobPL/kommit.git
cd kommit

# 빌드 및 설치
cargo install --path kommit
```

## 사용법

Kommit은 두 가지 주요 서브커맨드를 제공합니다: `run` (메시지 생성) 및 `config` (설정 관리).

### 1. 커밋 메시지 생성 (`run`)

스테이징된 변경 사항 또는 작업 트리의 변경 사항을 분석하여 커밋 메시지를 생성합니다.

```bash
# 기본 실행 (설정된 기본값 사용)
kommit run

# 스테이징된 변경 사항만 분석
kommit run --staged

# 특정 언어, 프로바이더, 모델 지정
kommit run --lang ko --provider ollama --model gemma2
```

#### `run` 주요 옵션

| 옵션               | 설명                                                                 | 기본값      |
|:-----------------|:-------------------------------------------------------------------|:---------|
| `--staged`       | 스테이징된 변경 사항(`git diff --staged`)만 분석합니다.                           | `false`  |
| `-m, --model`    | 사용할 LLM 모델 명칭을 지정합니다.                                              | `gemma4` |
| `-l, --lang`     | 메시지 언어를 지정합니다. (`en`, `ko`)                                        | `en`     |
| `-p, --provider` | AI 프로바이더를 지정합니다. (`ollama`, `lm-studio`)                           | `ollama` |
| `-s, --stream`   | 스트리밍 응답 활성화 여부를 지정합니다.                                             | `false`  |
| `-t, --think`    | 추론(Thinking) 레벨을 지정합니다. (`true`, `false`, `low`, `medium`, `high`) | `false`  |

### 2. 설정 관리 (`config`)

매번 명령행 옵션을 입력하지 않도록 기본 설정을 관리할 수 있습니다.

```bash
# 현재 설정 및 설정 파일 경로 확인
kommit config show

# 기본 모델 변경
kommit config set model llama3

# 기본 언어를 한국어로 변경
kommit config set lang ko

# 설정 디렉토리 열기
kommit config open
```

## 사전 요구 사항

이 앱은 외부 또는 로컬 LLM 프로바이더를 통해 메시지를 생성합니다. 선택한 프로바이더의 서비스가 활성화되어 있고 API 접근이 가능한 상태여야 합니다.

- **로컬 프로바이더 (Ollama, LM Studio 등)**: 해당 서비스가 로컬에서 실행 중이어야 하며, 기본 포트(11434, 1234 등)가 열려 있어야 합니다.
- **모델 준비**: 사용하려는 모델이 해당 프로바이더에 이미 설치 또는 다운로드되어 있어야 합니다.
- **네트워크 설정**: 향후 추가될 클라우드 기반 프로바이더의 경우 API 키 또는 인증 설정이 필요할 수 있습니다.

## 라이선스

이 프로젝트는 다음 두 라이선스 하에 배포됩니다:

- **Apache License, Version 2.0** ([LICENSE-APACHE](LICENSE-APACHE) 또는 http://www.apache.org/licenses/LICENSE-2.0)
- **MIT License** ([LICENSE-MIT](LICENSE-MIT) 또는 http://opensource.org/licenses/MIT)

사용자는 본인의 선택에 따라 위 두 라이선스 중 하나를 택하여 사용할 수 있습니다.

---

## 향후 계획 (Roadmap)

Kommit은 지속적으로 기능을 확장할 예정입니다:

- [ ] **자동 커밋 및 편집 모드**: 생성된 메시지로 즉시 커밋하거나, 커밋 전 사용자가 메시지를 수정할 수 있는 인터랙티브 모드 추가
- [x] **설정 파일 지원**: 매번 옵션을 입력하지 않아도 되도록 `config.toml`을 통한 기본값 설정 기능
- [ ] **커스텀 프롬프트 템플릿**: 조직 내의 특정 커밋 규칙을 적용할 수 있는 사용자 정의 템플릿 기능
- [ ] **성능 최적화**: 긴 Diff 처리를 위한 토큰 최적화 및 실시간 메시지 생성을 위한 스트리밍(Streaming) 지원
- [ ] **프로바이더 확장**: OpenAI, Anthropic 등 클라우드 기반 LLM 서비스 지원 및 API 키 관리 기능
