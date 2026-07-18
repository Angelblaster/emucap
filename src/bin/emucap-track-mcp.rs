//! emucap 추적 MCP(`emucap-track-mcp`) — 실험 기록 저장 서버.
//!
//! 이 서버는 `.emucap/`에 run을 저장하고 검색하며 에뮬레이터를 제어하지 않는다.
//! rom_sha1과 connection_ref는 제어 MCP의 get_rom_info/status에서 읽어 인자로 넘긴다.
//! 두 MCP는 서로 호출하지 않는다. 도구 동작은 `emucap::track::mcp_ops`에 있고, 여기서는
//! 현재 run 선택과 Value→CallToolResult 변환을 담당한다.

use std::path::Path;
use std::sync::{Arc, Mutex};

use rmcp::handler::server::{router::tool::ToolRouter, wrapper::Parameters};
use rmcp::model::{CallToolResult, Content, Implementation, ServerCapabilities, ServerInfo};
use rmcp::{tool, tool_handler, tool_router, ServerHandler, ServiceExt};
use schemars::JsonSchema;
use serde::Deserialize;

/// 추적 서버 상태 — link 없음(emulator-less). active_run만 in-memory로 들고, 원장 쓰기는
/// 모두 이 한 프로세스 안에서 직렬화된다(run.json RMW 동시성이 한 프로세스에 갇힘).
#[derive(Clone)]
struct EmucapTrack {
    active_run: Arc<Mutex<Option<ActiveRun>>>,
    tool_router: ToolRouter<EmucapTrack>,
}

/// in-memory 활성 run 바인딩. connection_ref는 제어 MCP에서 받아 넘긴 표식(어느 세션 run인지)일 뿐
/// 이 서버가 연결을 들고 있지 않다 — 자동 도출 없음.
#[derive(Clone)]
struct ActiveRun {
    rom_sha1: String,
    run_id: String,
    connection_ref: Option<String>,
}

// ── 도구 Args ────────────────────────────────────────────────────────────────

#[derive(Deserialize, JsonSchema)]
struct TrackRunStartArgs {
    /// 필수 — 제어 MCP `get_rom_info`의 균일 `rom_sha1` 필드를 받아 전달한다(어댑터별 해시를 정규화한 불투명한 그룹 식별자). 이 MCP는 에뮬레이터를 모르므로 추론하지 않는다.
    rom_sha1: String,
    /// 선택 — 어느 세션/연결의 run인지 표식(제어 MCP `status.emulator_identity.name` 또는 `"port:"`+
    /// `status.listening_port`). 같은 connection_ref의 직전 미종료 run을 자동 마감(superseded)하는 데 쓴다.
    #[serde(default)]
    connection_ref: Option<String>,
    #[serde(default)]
    goal: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
}

#[derive(Deserialize, JsonSchema)]
struct RunResumeArgs {
    /// 재바인딩할 run_id(전역 유일). 디스크에서 status=running일 때만 resume된다(종료된 run은 새 run_start로).
    run_id: String,
}

#[derive(Deserialize, JsonSchema)]
struct RunFinishArgs {
    /// done|aborted|error (기본 done)
    #[serde(default)]
    status: Option<String>,
    /// 특정 run을 id로 종료(전역 유일). 생략 시 활성 run. 서버 재시작 등으로 고아화된 run 복구용.
    #[serde(default)]
    run_id: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
struct LogMetricArgs {
    key: String,
    value: f64,
}

#[derive(Deserialize, JsonSchema)]
struct LogGateArgs {
    name: String,
    /// machine | judgment
    kind: String,
    passed: Option<bool>,
    evidence_ref: Option<String>,
    detail: Option<String>,
    case_ref: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
struct LogArtifactArgs {
    kind: String,
    /// 이미 캡처된 파일 경로. 상대경로는 작업 repo git root 기준으로 해소된다(MCP 서버 cwd 아님).
    path: String,
}

#[derive(Deserialize, JsonSchema)]
struct SetReproArgs {
    base: Option<String>,
    movie_ref: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
struct LogFindingArgs {
    /// 생략 시 활성 run의 rom_sha1. 둘 다 없으면 에러.
    rom_sha1: Option<String>,
    claim: String,
    #[serde(default)]
    evidence_refs: Vec<String>,
    #[serde(default)]
    promoted: bool,
}

#[derive(Deserialize, JsonSchema)]
struct LogInterventionArgs {
    /// 개입 종류 — write_memory|load_state|reset|input_burst 등 자유 라벨. 제어 MCP가 더는 자동
    /// 기록하지 않으므로 에이전트가 상태변경을 직접 기록해 repro_status 충실도를 유지한다.
    op: String,
    /// 개입의 구조화 인자(예: write_memory면 {memory_type,address,hex}). 생략 시 null.
    #[serde(default)]
    args: Option<serde_json::Value>,
    /// 개입 시점 프레임(선택).
    #[serde(default)]
    at_frame: Option<u64>,
    /// 개입을 유발한 이벤트 참조(선택).
    #[serde(default)]
    at_event: Option<String>,
    /// frozen 컨텍스트에서의 개입이면 true(기본 false).
    #[serde(default)]
    frozen_context: bool,
}

#[derive(Deserialize, JsonSchema)]
struct QueryRunsArgs {
    rom_sha1: Option<String>,
    goal: Option<String>,
    status: Option<String>,
    /// 결과를 이 경로에 JSON으로 저장하고 요약만 반환(큰 결과가 예상될 때 context 절약). 생략 시 인라인.
    #[serde(default)]
    output_path: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
struct GetRunArgs {
    rom_sha1: String,
    run_id: String,
}

#[derive(Deserialize, JsonSchema)]
struct CompareRunsArgs {
    /// 비교 기준 run_id(A)
    run_id_a: String,
    /// 비교 대상 run_id(B)
    run_id_b: String,
}

#[derive(Deserialize, JsonSchema)]
struct SummarizeRunsArgs {
    /// goal 정확 일치 필터(생략 시 무제약)
    #[serde(default)]
    goal: Option<String>,
    /// tag 정확 원소 일치 필터(생략 시 무제약)
    #[serde(default)]
    tag: Option<String>,
    /// rom_sha1 필터(생략 시 무제약)
    #[serde(default)]
    rom_sha1: Option<String>,
    /// 결과를 이 경로에 JSON으로 저장하고 요약만 반환(큰 결과가 예상될 때 context 절약). 생략 시 인라인.
    #[serde(default)]
    output_path: Option<String>,
}

// ── 공통 헬퍼 ────────────────────────────────────────────────────────────────

/// 추적 도구 공통: ok json
fn track_ok(v: serde_json::Value) -> CallToolResult {
    CallToolResult::success(vec![Content::text(v.to_string())])
}
/// 추적 도구 공통: 에러 텍스트
fn track_err(msg: impl std::fmt::Display) -> CallToolResult {
    let mut r = CallToolResult::success(vec![Content::text(format!("{msg}"))]);
    r.is_error = Some(true);
    r
}

/// MCP 서버 사용 가이드. 에이전트가 항상 보는 유일한 문서이므로 자기완결적이어야 한다.
const SERVER_INSTRUCTIONS: &str = r#"emucap 실험 추적 MCP — 시도를 기록·재현·비교해 어떤 조건에서 패치가 성공하는지 찾기 위한 `.emucap/` 기록 저장소다. **이 서버는 에뮬레이터를 제어하지 않는다.** 메모리·상태·화면·입력 제어는 별도 제어 MCP(emucap-mcp)에서 한다. 두 서버는 서로 호출하지 않으므로 에이전트가 필요한 값을 직접 전달한다.

[제어 MCP에서 받을 값]
  • rom_sha1: 이 MCP는 ROM을 읽지 않는다. 제어 MCP `get_rom_info`가 반환한 공통 `rom_sha1` 값을 run_start/get_run/query_runs/log_finding에 그대로 넘긴다. `rom_sha1`이 없는 경우에만 `shasum -a1 <content>`를 쓴다.
  • connection_ref(선택): 제어 MCP `status.emulator_identity.name`, 또는 `"port:" + status.listening_port`. run_start에 넘기면 같은 connection의 직전 미종료 run을 자동 마감(superseded)한다.
  • regression_run/verify_determinism은 제어 MCP가 에뮬레이터를 실행해 결과만 반환한다. 그 결과를 log_gate/log_metric으로 기록한다. 프레임 경계 탐색 결과도 같은 방식으로 기록한다.
  • write_memory/load_state/reset/입력처럼 상태를 바꾸는 호출은 자동 기록되지 않는다. 다시 재현할 수 있도록 log_intervention으로 기록한다.

[저장 위치] EMUCAP_TRACK_ROOT가 있으면 그 경로, 없으면 작업 중인 git 저장소의 `.emucap`, git 저장소가 아니면 현재 디렉터리의 `.emucap`을 쓴다. bootstrap은 실제 경로를 ledger_path로, 선택 이유를 ledger_path_source로 반환한다. 현재 디렉터리를 쓴 경우에는 ledger_path_warning도 반환하므로 EMUCAP_TRACK_ROOT를 지정하거나 git 저장소에서 실행하는 편이 안전하다. run.json은 이 서버만 쓰게 하고, 라이브 세션 중에는 `emucap track import`처럼 별도 프로세스가 쓰는 명령을 함께 실행하지 않는다. broker를 여러 세션에서 쓸 때는 세션별 EMUCAP_TRACK_ROOT를 나누는 것이 안전하다.

[run 수명]
  run_start(rom_sha1 필수, connection_ref/goal/description/tags 선택): 새 run을 현재 기록 대상으로 지정하고 {run_id, rom_sha1, ledger_path}를 반환한다. 같은 connection_ref와 rom_sha1을 가진 미종료 run이 이미 있으면 새로 만들지 않고 이어 쓰며 resumed:true를 반환한다. rom_sha1이 다르면 같은 connection_ref의 이전 run을 superseded로 끝내고 새 run을 만든다.
  run_resume(run_id): 지정한 running run을 현재 기록 대상으로 다시 선택하고 resumed:true를 반환한다. MCP 재연결 뒤 bootstrap의 running_runs에서 이어 쓸 run을 찾았을 때 사용한다. 이미 종료된 run이면 오류다.
  log_metric/log_gate/log_artifact/set_reproduction/log_intervention은 현재 run이 있어야 한다. 없으면 run_start 또는 run_resume을 먼저 호출한다. log_finding은 현재 run이 있거나 rom_sha1을 직접 주면 기록할 수 있다.
  run_finish(status=done|aborted|error 기본 done, run_id 선택): run_id가 있으면 현재 선택 여부와 상관없이 그 run을 종료한다. 생략하면 현재 run을 종료한다. 이어 쓸 run에는 run_finish를 쓰지 않는다. 더 이상 쓰지 않을 미종료 run만 종료한다.

[기록 도구]
  log_metric(key, value): 이름과 숫자 한 쌍을 기록한다.
  log_gate(name, kind=machine|judgment, passed?/evidence_ref?/detail?/case_ref?): 검증 결과를 기록한다. passed를 생략하면 pending이다. 제어 MCP 분석 도구의 결과는 주로 여기에 기록한다.
  log_artifact(kind, path): 이미 캡처된 파일을 등록하고 sha256을 계산한다. 새 캡처를 만들지는 않는다. 상대경로는 작업 중인 git 저장소에서 찾는다.
  set_reproduction(base?, movie_ref?): 현재 run을 다시 실행하는 데 쓸 base와 movie_ref를 설정한다. repro_status는 자동으로 계산된다.
  log_finding(claim, rom_sha1?/evidence_refs?/promoted?): 발견을 해당 ROM에 기록한다. promoted=true는 확정된 발견으로 표시한다.
  log_intervention(op, args?/at_frame?/at_event?/frozen_context?): 현재 run에 상태 변경 이력을 기록한다.

[저장된 기록 읽기]
  query_runs(rom_sha1?/goal?/status?): 필터로 run 목록(최근 우선). 손상 JSON은 skipped로 세고 죽지 않는다.
  get_run(rom_sha1, run_id): 저장된 run.json 내용과 ledger_path를 반환한다. run_id는 전체에서 고유하지만 ROM별 디렉터리에서 찾으므로 rom_sha1도 필요하다.
  compare_runs(run_id_a, run_id_b): 두 run의 수치 변화, log_gate 결과 변화, 재현성, 상태 변경, 파일 차이를 반환한다.
  summarize_runs(goal?/tag?/rom_sha1?): 여러 run의 상태·재현성 분포, log_gate 결과 비율, 상태 변경 종류, run별 요약을 반환한다.
  **성공 여부를 대신 결정하지 않는다. 저장된 결과를 보고 어떤 조건에서 성공했는지는 에이전트가 판단한다.**

[셸 CLI] emucap track ls|show|compare|summarize|reindex|import도 같은 `.emucap/` 기록을 읽는다.

[큰 결과] query_runs/summarize_runs에는 output_path를 줘 파일로 저장하고 요약과 경로만 받는다. 메모리 전체 덤프는 제어 MCP의 dump_memory를 쓴다. 이 추적 MCP에는 dump_memory가 없다."#;

// ── 도구 구현 ────────────────────────────────────────────────────────────────

#[tool_router(router = tool_router)]
impl EmucapTrack {
    fn new() -> Self {
        Self {
            active_run: Arc::new(Mutex::new(None)),
            tool_router: Self::tool_router(),
        }
    }

    /// 활성 run에 mcp_ops를 적용하는 공통 래퍼(UlidGen·now·root 주입). 로직은 lib(mcp_ops)에 있고
    /// 여기선 active_run 상태 해소 + Value→CallToolResult 변환만 한다.
    fn with_active<F>(&self, f: F) -> CallToolResult
    where
        F: FnOnce(
            &Path,
            &ActiveRun,
            &emucap::track::id::UlidGen,
            &str,
        ) -> Result<serde_json::Value, String>,
    {
        let active = self
            .active_run
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let Some(ar) = active else {
            return track_err("활성 run 없음 — run_start 먼저");
        };
        let root = emucap::track::store::root_from_env();
        let now = emucap::track::clock::now_rfc3339();
        match f(&root, &ar, &emucap::track::id::UlidGen, &now) {
            Ok(v) => track_ok(v),
            Err(e) => track_err(e),
        }
    }

    /// resume 공통: binding을 in-memory active로 재바인딩한다(supersede+새 run이 아니라 디스크의
    /// still-running run을 다시 active로 잡는 것이라 새 run을 만들지 않는다). 드물게 다른 active가
    /// 이미 바인딩돼 있으면 그 run을 aborted(superseded)로 마감해 단일-active 불변식을 지킨다.
    /// 반환에 `resumed:true`. run_start의 resume 경로와 run_resume가 공유한다.
    fn rebind_active(
        &self,
        root: &Path,
        now: &str,
        binding: emucap::track::mcp_ops::ResumeBinding,
        caller_supplied_meta: bool,
    ) -> CallToolResult {
        let mut g = self.active_run.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(prev) = g.as_ref() {
            if prev.run_id != binding.run_id {
                let _ = emucap::track::ops::finish_run(
                    root,
                    &prev.rom_sha1,
                    &prev.run_id,
                    emucap::track::model::RunStatus::Aborted,
                    now,
                );
            }
        }
        let mut resp = serde_json::json!({
            "run_id": binding.run_id.clone(),
            "rom_sha1": binding.rom_sha1.clone(),
            "ledger_path": root.display().to_string(),
            "resumed": true,
        });
        // 침묵 폐기 방지: resume는 기존 run 메타를 유지하므로, 이 호출이 넘긴 goal/description/tags는
        // 적용되지 않는다 — 응답에 명시해 "새 goal로 새 실험" 의도가 옛 run에 흡수되는 걸 가시화한다.
        if caller_supplied_meta {
            resp["note"] = serde_json::json!("기존 run을 resume했다 — 이 호출의 goal/description/tags는 무시됐다(기존 run 메타 유지). 새 goal로 *새 실험*을 시작하려면 run_finish 후 run_start하라.");
        }
        *g = Some(ActiveRun {
            rom_sha1: binding.rom_sha1,
            run_id: binding.run_id,
            connection_ref: binding.connection_ref,
        });
        track_ok(resp)
    }

    #[tool(
        description = "추적 MCP의 첫 진입점. 이 서버는 에뮬레이터를 제어하지 않고 `.emucap/`에 실험 기록을 저장한다. ledger_path, 현재 선택한 run, 저장된 미종료 run, 사용할 수 있는 기록·검색 방법을 반환한다. rom_sha1은 제어 MCP(emucap-mcp)의 get_rom_info에서 읽어 run_start에 넘긴다"
    )]
    async fn bootstrap(&self) -> CallToolResult {
        track_ok(self.make_bootstrap_value())
    }

    #[tool(
        description = "실험 Run을 시작한다(메타 전용, 에뮬레이터 무통신). rom_sha1은 필수 — 제어 MCP의 get_rom_info에서 읽어 전달하라(이 MCP는 에뮬레이터를 모른다). connection_ref는 선택(어느 세션 run인지 표식). **resume**: connection_ref가 있고 디스크에 그 connection_ref + 같은 rom의 still-running run이 있으면 새 run을 만들지 않고 그 run을 active로 재바인딩한다(반환 resumed:true) — /mcp 재연결로 active가 끊겨도 같은 run을 이어써 파편화를 막는다. rom이 다르면 같은 connection_ref의 직전 미종료 run을 자동 마감하고 새 run을 만든다. 이후 log_*가 이 run에 기록된다."
    )]
    async fn run_start(&self, Parameters(a): Parameters<TrackRunStartArgs>) -> CallToolResult {
        let root = emucap::track::store::root_from_env();
        let now = emucap::track::clock::now_rfc3339();
        // resume(재연결 복원): connection_ref가 있고 디스크에 그 connection_ref + 같은 rom의 still-running
        // run이 있으면 supersede+새 run이 아니라 그 run을 active로 재바인딩한다(파편화 0). rom이 다르거나
        // 일치 running이 없으면 None → 아래 supersede 경로(start_run의 finish_stale_running)가 직전 run을
        // 마감하고 새 run을 만든다. best-effort: 조회 에러는 fall-through해 start_run이 노출한다.
        if let Some(cref) = a.connection_ref.as_deref() {
            if let Ok(Some(binding)) =
                emucap::track::mcp_ops::find_resumable_run(&root, cref, &a.rom_sha1)
            {
                return self.rebind_active(
                    &root,
                    &now,
                    binding,
                    a.goal.is_some() || a.description.is_some() || !a.tags.is_empty(),
                );
            }
        }
        // 원장 위생: 새 run 전 직전 in-memory 활성 run을 aborted(superseded)로 정리한다.
        // 같은 connection의 디스크 고아 running 정리(서버 재시작 복구)는 mcp_ops::start_run이 맡는다.
        if let Some(ar) = self
            .active_run
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            let _ = emucap::track::ops::finish_run(
                &root,
                &ar.rom_sha1,
                &ar.run_id,
                emucap::track::model::RunStatus::Aborted,
                &now,
            );
        }
        match emucap::track::mcp_ops::start_run(
            &root,
            &emucap::track::id::UlidGen,
            &now,
            &a.rom_sha1,
            a.connection_ref.clone(),
            a.goal,
            a.description,
            a.tags,
        ) {
            Ok(v) => {
                // start_run이 만든 run_id로 active를 바인딩한다. run_id가 없으면(있을 수 없는 내부
                // 불변식 위반) 조용히 성공시키지 않고 에러로 노출한다.
                match v.get("run_id").and_then(|s| s.as_str()) {
                    Some(run_id) => {
                        *self.active_run.lock().unwrap_or_else(|e| e.into_inner()) =
                            Some(ActiveRun {
                                rom_sha1: a.rom_sha1.clone(),
                                run_id: run_id.to_string(),
                                connection_ref: a.connection_ref,
                            });
                        track_ok(v)
                    }
                    None => track_err("내부 오류: start_run 응답에 run_id 없음"),
                }
            }
            Err(e) => track_err(e),
        }
    }

    #[tool(
        description = "특정 running Run을 in-memory active로 다시 바인딩한다(resume). /mcp 재연결 등으로 active 바인딩이 끊겼을 때, bootstrap의 running_runs에서 이 세션 run을 골라 run_id로 이어쓴다 — 새 run을 만들지 않아 파편화가 없다(반환 resumed:true). status가 running이 아니면(이미 종료) 에러. connection_ref가 있으면 run_start(같은 connection_ref)로도 같은 resume이 일어난다."
    )]
    async fn run_resume(&self, Parameters(a): Parameters<RunResumeArgs>) -> CallToolResult {
        let root = emucap::track::store::root_from_env();
        let now = emucap::track::clock::now_rfc3339();
        match emucap::track::mcp_ops::resume_run_by_id(&root, &a.run_id) {
            Ok(binding) => self.rebind_active(&root, &now, binding, false),
            Err(e) => track_err(e),
        }
    }

    #[tool(
        description = "활성 Run을 종료한다(status=done|aborted|error). run_id를 주면 활성 상태와 무관하게 그 run을 직접 종료한다(서버 재시작 등으로 고아화된 running run 복구용). run_start는 새 run 시작 시 같은 연결의 직전 미종료 run을 자동 마감하므로 보통은 명시 종료만 신경쓰면 된다."
    )]
    async fn run_finish(&self, Parameters(a): Parameters<RunFinishArgs>) -> CallToolResult {
        let status =
            match emucap::track::mcp_ops::parse_run_status(a.status.as_deref().unwrap_or("done")) {
                Ok(s) => s,
                Err(e) => return track_err(e),
            };
        let root = emucap::track::store::root_from_env();
        let now = emucap::track::clock::now_rfc3339();
        // run_id 지정: in-memory 활성 상태에 의존하지 않고 디스크에서 직접 종료(서버 재시작 등 고아 복구).
        if let Some(rid) = a.run_id.as_deref() {
            return match emucap::track::mcp_ops::finish_run_by_id(&root, rid, status, &now) {
                Ok(v) => {
                    if let Some(id) = v.get("finished").and_then(|s| s.as_str()) {
                        let mut g = self.active_run.lock().unwrap_or_else(|e| e.into_inner());
                        if g.as_ref().map(|ar| ar.run_id == id).unwrap_or(false) {
                            *g = None;
                        }
                    }
                    track_ok(v)
                }
                Err(e) => track_err(e),
            };
        }
        let active = self
            .active_run
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let Some(ar) = active else {
            return track_err("활성 run 없음 — run_start 먼저(또는 run_id로 특정 run 종료)");
        };
        match emucap::track::mcp_ops::finish_active_run(
            &root,
            &ar.rom_sha1,
            &ar.run_id,
            status,
            &now,
        ) {
            Ok(v) => {
                *self.active_run.lock().unwrap_or_else(|e| e.into_inner()) = None;
                track_ok(v)
            }
            Err(e) => track_err(e),
        }
    }

    #[tool(description = "활성 Run에 정량 메트릭을 기록한다(메타 전용).")]
    async fn log_metric(&self, Parameters(a): Parameters<LogMetricArgs>) -> CallToolResult {
        self.with_active(|root, ar, gen, now| {
            emucap::track::mcp_ops::log_metric(
                root,
                &ar.rom_sha1,
                &ar.run_id,
                gen,
                now,
                &a.key,
                a.value,
            )
        })
    }

    #[tool(
        description = "현재 Run에 검증 결과를 기록한다(kind=machine|judgment, passed 생략=pending). 제어 MCP의 분석 결과는 주로 이 도구로 남긴다."
    )]
    async fn log_gate(&self, Parameters(a): Parameters<LogGateArgs>) -> CallToolResult {
        // kind 검증을 active 검사보다 먼저(에러 우선순위 보존) — 로직은 mcp_ops::log_gate가 재검증·기록.
        if let Err(e) = emucap::track::mcp_ops::parse_gate_kind(&a.kind) {
            return track_err(e);
        }
        self.with_active(|root, ar, gen, now| {
            emucap::track::mcp_ops::log_gate(
                root,
                &ar.rom_sha1,
                &ar.run_id,
                gen,
                now,
                &a.name,
                &a.kind,
                a.passed,
                a.evidence_ref.clone(),
                a.detail.clone(),
                a.case_ref.clone(),
            )
        })
    }

    #[tool(
        description = "이미 캡처된 파일을 활성 Run의 artifact로 등록한다(sha256 계산, 새 캡처 안 함)."
    )]
    async fn log_artifact(&self, Parameters(a): Parameters<LogArtifactArgs>) -> CallToolResult {
        let active = self
            .active_run
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let Some(ar) = active else {
            return track_err("활성 run 없음 — run_start 먼저");
        };
        let root = emucap::track::store::root_from_env();
        // 상대경로는 MCP 서버 cwd가 아니라 *작업 repo* 루트 기준으로 해소(최소놀람·재현성).
        let git_root = emucap::track::store::nearest_git_root();
        match emucap::track::mcp_ops::log_artifact(
            &root,
            &ar.rom_sha1,
            &ar.run_id,
            &emucap::track::id::UlidGen,
            &a.kind,
            Path::new(&a.path),
            git_root.as_deref(),
            None,
        ) {
            Ok(v) => track_ok(v),
            Err(e) => track_err(e),
        }
    }

    #[tool(description = "활성 Run의 재현 base/movie를 설정한다(repro_status는 자동 도출).")]
    async fn set_reproduction(&self, Parameters(a): Parameters<SetReproArgs>) -> CallToolResult {
        self.with_active(|root, ar, _gen, _now| {
            emucap::track::mcp_ops::set_reproduction(
                root,
                &ar.rom_sha1,
                &ar.run_id,
                a.base.clone(),
                a.movie_ref.clone(),
            )
        })
    }

    #[tool(
        description = "현재 Run에 상태 변경을 기록한다(op=write_memory|load_state|reset|input_burst 등). 제어 MCP가 자동으로 남기지 않으므로 나중에 같은 실행을 재현하려면 에이전트가 직접 기록한다."
    )]
    async fn log_intervention(
        &self,
        Parameters(a): Parameters<LogInterventionArgs>,
    ) -> CallToolResult {
        self.with_active(|root, ar, gen, now| {
            emucap::track::mcp_ops::log_intervention(
                root,
                &ar.rom_sha1,
                &ar.run_id,
                gen,
                now,
                a.at_frame,
                a.at_event.clone(),
                a.frozen_context,
                &a.op,
                a.args.clone().unwrap_or(serde_json::Value::Null),
            )
        })
    }

    #[tool(
        description = "발견을 ROM 스코프로 기록한다(promoted=true면 승격). rom_sha1 생략 시 활성 run의 것을 쓴다."
    )]
    async fn log_finding(&self, Parameters(a): Parameters<LogFindingArgs>) -> CallToolResult {
        let active = self
            .active_run
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let rom_sha1 = match a
            .rom_sha1
            .clone()
            .or_else(|| active.as_ref().map(|r| r.rom_sha1.clone()))
        {
            Some(s) => s,
            None => return track_err("rom_sha1 미지정 + 활성 run 없음"),
        };
        let run_id = active.as_ref().map(|r| r.run_id.clone());
        let root = emucap::track::store::root_from_env();
        let now = emucap::track::clock::now_rfc3339();
        match emucap::track::mcp_ops::log_finding(
            &root,
            &rom_sha1,
            &emucap::track::id::UlidGen,
            &now,
            &a.claim,
            run_id,
            a.evidence_refs,
            a.promoted,
        ) {
            Ok(v) => track_ok(v),
            Err(e) => track_err(e),
        }
    }

    #[tool(description = "저장된 Run을 검색한다(rom_sha1/goal/status 필터).")]
    async fn query_runs(&self, Parameters(a): Parameters<QueryRunsArgs>) -> CallToolResult {
        let root = emucap::track::store::root_from_env();
        match emucap::track::mcp_ops::query_runs(
            &root,
            emucap::track::query::RunFilter {
                rom_sha1: a.rom_sha1,
                goal: a.goal,
                status: a.status,
            },
        ) {
            Ok(v) => match a.output_path.as_deref() {
                Some(p) => match emucap::offload::offload_result(&v, std::path::Path::new(p)) {
                    Ok(s) => track_ok(s),
                    Err(e) => track_err(e),
                },
                None => track_ok(v),
            },
            Err(e) => track_err(e),
        }
    }

    #[tool(description = "저장된 Run의 상세 내용(run.json)을 반환한다.")]
    async fn get_run(&self, Parameters(a): Parameters<GetRunArgs>) -> CallToolResult {
        let root = emucap::track::store::root_from_env();
        match emucap::track::mcp_ops::get_run(&root, &a.rom_sha1, &a.run_id) {
            Ok(v) => track_ok(v),
            Err(e) => track_err(e),
        }
    }

    #[tool(
        description = "두 run을 비교해 수치 변화, 검증 결과 변화, 재현 가능성, 상태 변경, 파일 차이를 반환한다. 에뮬레이터에는 요청을 보내지 않는다. run_id는 전체 기록에서 고유하다. 같은 이름의 gates/metrics가 여러 번 기록됐으면 마지막 값을 대표로 고르고 발생 횟수도 함께 반환한다."
    )]
    async fn compare_runs(&self, Parameters(a): Parameters<CompareRunsArgs>) -> CallToolResult {
        let root = emucap::track::store::root_from_env();
        match emucap::track::mcp_ops::compare_runs(&root, &a.run_id_a, &a.run_id_b) {
            Ok(v) => track_ok(v),
            Err(e) => track_err(e),
        }
    }

    #[tool(
        description = "goal/tag/rom으로 묶은 run들을 요약한다: 상태와 재현 가능성 분포, 검증 항목별 통과·실패·미결 수, 상태 변경 종류, 수치 이름, run별 요약을 반환한다. 에뮬레이터에는 요청을 보내지 않고 성공 여부도 대신 판단하지 않는다. 손상된 run은 건너뛰고 skipped로 센다."
    )]
    async fn summarize_runs(&self, Parameters(a): Parameters<SummarizeRunsArgs>) -> CallToolResult {
        let root = emucap::track::store::root_from_env();
        let filter = emucap::track::summary::SummaryFilter {
            goal: a.goal,
            tag: a.tag,
            rom_sha1: a.rom_sha1,
        };
        match emucap::track::mcp_ops::summarize_runs(&root, filter) {
            Ok(v) => match a.output_path.as_deref() {
                Some(p) => match emucap::offload::offload_result(&v, std::path::Path::new(p)) {
                    Ok(s) => track_ok(s),
                    Err(e) => track_err(e),
                },
                None => track_ok(v),
            },
            Err(e) => track_err(e),
        }
    }
}

impl EmucapTrack {
    /// bootstrap 응답 생성: ledger_path, 현재 run, 저장된 미종료 run, 기록·검색 안내를 담는다.
    /// running run 검색은 best-effort이며 저장소가 없거나 손상돼도 bootstrap은 성공한다.
    fn make_bootstrap_value(&self) -> serde_json::Value {
        let (root, root_source) = emucap::track::store::root_from_env_with_source();
        let active = self
            .active_run
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let active_json = match &active {
            Some(ar) => serde_json::json!({
                "rom_sha1": ar.rom_sha1,
                "run_id": ar.run_id,
                "connection_ref": ar.connection_ref,
            }),
            None => serde_json::Value::Null,
        };
        // 디스크의 미종료(running) run을 노출해 고아 복구(run_finish(run_id))를 돕는다. best-effort.
        let running = match emucap::track::mcp_ops::query_runs(
            &root,
            emucap::track::query::RunFilter {
                status: Some("running".into()),
                ..Default::default()
            },
        ) {
            Ok(v) => v.get("runs").cloned().unwrap_or(serde_json::json!([])),
            Err(_) => serde_json::json!([]),
        };
        let mut out = serde_json::json!({
            "ok": true,
            "start_here": true,
            "first_tool": "bootstrap",
            "server": "emucap-track-mcp",
            "emulator_less": true,
            "ledger_path": root.display().to_string(),
            "ledger_path_source": root_source.as_str(),
            "ledger_root_env": "EMUCAP_TRACK_ROOT",
            "active_run": active_json,
            "running_runs": running,
            "assembly": {
                "note": "이 MCP는 에뮬레이터를 모른다. rom_sha1·connection_ref는 제어 MCP(emucap-mcp)에서 읽어 넘긴다.",
                "rom_sha1": "제어 MCP `get_rom_info`의 균일 `rom_sha1` 필드로 구해 run_start에 넘겨라(없는 백엔드만 `shasum -a1 <content>`)",
                "connection_ref": "제어 MCP status의 connection 이름 또는 \"port:N\"(선택; 같은 connection + 같은 rom의 still-running run은 run_start가 새 run 대신 resume한다)",
                "analysis_verbs": "regression_run/verify_determinism은 제어 MCP가 결과를 반환만 한다 — 그 결과를 log_gate/log_metric으로 여기 기록하라",
                "interventions": "write_memory/load_state/reset/입력은 제어 MCP가 자동 기록하지 않는다 — log_intervention으로 명시 기록하라"
            },
            "supported_queries": ["query_runs", "get_run", "compare_runs", "summarize_runs"],
            "resume": "재연결로 active_run이 끊겼으면 running_runs에서 이 세션 run을 골라 run_resume(run_id=...)로 이어쓴다(또는 같은 connection_ref로 run_start하면 자동 resume). 새 run을 만들지 않아 파편화가 없다.",
            "orphan_recovery": "정말 죽은 고아만 run_finish(run_id=...)로 종료한다(이어쓸 run은 resume, 버릴 run만 finish).",
            "next_action": "active_run이 null이고 running_runs에 이 세션 run이 있으면 run_resume(run_id=...)로 이어쓴다. 없으면 run_start(rom_sha1=...)로 시작한다. 진짜 고아만 run_finish로 정리한다."
        });
        // ledger 경로 모호 케이스: cwd_fallback이면 위치가 서버 cwd에 의존하니 경고를 단다.
        if let Some(w) = root_source.warning() {
            if let Some(obj) = out.as_object_mut() {
                obj.insert(
                    "ledger_path_warning".into(),
                    serde_json::Value::String(w.to_string()),
                );
            }
        }
        out
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for EmucapTrack {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                "emucap-track-mcp",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(SERVER_INSTRUCTIONS)
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let server = EmucapTrack::new();
    let service = server.serve(rmcp::transport::stdio()).await?;
    service.waiting().await?;
    Ok(())
}

#[cfg(test)]
// 테스트는 프로세스 전역 env(EMUCAP_TRACK_ROOT)를 직렬화하려 ENV_LOCK 가드를 .await 너머로 든다.
// tokio::test는 current-thread 런타임이고 추적 도구 future는 yield하지 않아 실제 경합은 없다 — 의도된 lint.
#[allow(clippy::await_holding_lock)]
#[path = "tests/emucap_track_mcp_tests.rs"]
mod tests;
