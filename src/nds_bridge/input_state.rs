use super::*;

impl<G: GdbTransport> NdsBridge<G> {
    /// Capture both DS screens (256x384, top over bottom) as a PNG. The DeSmuME fork encodes
    /// the native RGB555 frame buffer and returns it base64-encoded over the ARM9 connection.
    pub(super) fn screenshot(&mut self) -> NdsResult<Value> {
        let b64 = self.arm9.screenshot_b64()?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64.trim())
            .map_err(|err| {
                // send_b64_reply()가 stray stop을 이미 걸러낸 뒤에도 decode가 실패하면 다른 원인이다(잘림 등).
                // 응답 길이와 앞부분을 실어 재발 시 진단 가능하게 한다 — stop이면 "S.."/"T..", 잘렸으면 len%4≠0.
                let t = b64.trim();
                let head: String = t.chars().take(32).collect();
                NdsBridgeError::Emulator(format!(
                    "screenshot: base64 decode failed: {err} (reply_len={}, head={head:?})",
                    t.len()
                ))
            })?;
        if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            return Err(NdsBridgeError::Emulator(
                "screenshot: DeSmuME reply was not a PNG".into(),
            ));
        }
        Ok(json!({
            "png_base64": b64,
            "format": "png",
            "width": 256,
            "height": 384,
        }))
    }

    /// Force a held button set until the next input command (empty list releases). Input is
    /// injected on the ARM9 connection (the primary CPU) and applied every frame by the fork.
    pub(super) fn set_input(&mut self, params: &Value) -> NdsResult<Value> {
        require_input_port_zero(params)?;
        let (mask, buttons) = buttons_to_mask(params.get("buttons"))?;
        self.arm9.send_input(mask, None)?;
        Ok(json!({
            "buttons": buttons,
            "cpu": "arm9",
            "override_engaged": mask != 0,
        }))
    }

    /// Hold a button set for `frames` processed frames, then auto-release. The fork counts the
    /// frames down itself, so the hold survives the frontend's per-frame input reset while the
    /// emulator runs.
    pub(super) fn press_buttons(&mut self, params: &Value) -> NdsResult<Value> {
        require_input_port_zero(params)?;
        let (mask, buttons) = buttons_to_mask(params.get("buttons"))?;
        if mask == 0 {
            return Err(NdsBridgeError::BadParams(
                "press_buttons requires at least one button".into(),
            ));
        }
        let frames = optional_num(params, "frames")?.unwrap_or(1).max(1);
        if frames > MAX_SYNC_TIMED_INPUT_FRAMES {
            return Err(NdsBridgeError::BadParams(format!(
                "NDS synchronous press_buttons supports at most {MAX_SYNC_TIMED_INPUT_FRAMES} frames; use set_input plus an explicit set_input([]) release for a longer hold"
            )));
        }
        let was_frozen = self.arm9.frozen;
        self.arm9.send_input(mask, Some(frames))?;
        if was_frozen {
            if let Err(err) = self.resume(&json!({})) {
                return Err(cleanup_timed_override_error(
                    err,
                    self.arm9.send_input(0, None),
                ));
            }
        }
        let terminal = match self.arm9.wait_timed_override("qEmucap,inputstatus", frames) {
            Ok(terminal) => terminal,
            Err(err) => {
                return Err(cleanup_timed_override_error(
                    err,
                    self.arm9.send_input(0, None),
                ))
            }
        };
        match terminal {
            TimedOverrideTerminal::Completed => Ok(json!({
                "status": "completed",
                "buttons": buttons,
                "frames": frames,
                "frames_elapsed": frames,
                "cpu": "arm9",
                "state": "running",
                "override_engaged": false,
            })),
            TimedOverrideTerminal::Interrupted { frames_elapsed } => {
                self.arm9.send_input(0, None)?;
                Ok(json!({
                    "status": "interrupted",
                    "reason": "breakpoint",
                    "buttons": buttons,
                    "frames": frames,
                    "frames_elapsed": frames_elapsed,
                    "cpu": "arm9",
                    "state": "frozen",
                    "override_engaged": false,
                }))
            }
        }
    }

    /// Touch the bottom screen at (x, y) (256x192). `release: true` lifts; `frames` presses for that
    /// many frames then auto-lifts (a tap); omitting both holds the press until the next touch command.
    pub(super) fn touch(&mut self, params: &Value) -> NdsResult<Value> {
        require_input_port_zero(params)?;
        if params
            .get("release")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            self.arm9.send_touch_release()?;
            return Ok(json!({ "released": true, "cpu": "arm9", "override_engaged": false }));
        }
        let x = optional_num(params, "x")?
            .ok_or_else(|| NdsBridgeError::BadParams("touch requires x (0-255)".into()))?;
        let y = optional_num(params, "y")?
            .ok_or_else(|| NdsBridgeError::BadParams("touch requires y (0-191)".into()))?;
        if x > 255 || y > 191 {
            return Err(NdsBridgeError::BadParams(format!(
                "touch out of range: x 0-255, y 0-191 (got x={x}, y={y})"
            )));
        }
        let frames = optional_num(params, "frames")?;
        if let Some(frames) = frames {
            if frames == 0 || frames > MAX_SYNC_TIMED_INPUT_FRAMES {
                return Err(NdsBridgeError::BadParams(format!(
                    "NDS timed touch frames must be 1..={MAX_SYNC_TIMED_INPUT_FRAMES}; omit frames for a persistent hold"
                )));
            }
            let was_frozen = self.arm9.frozen;
            self.arm9.send_touch(x as u16, y as u16, Some(frames))?;
            if was_frozen {
                if let Err(err) = self.resume(&json!({})) {
                    return Err(cleanup_timed_override_error(
                        err,
                        self.arm9.send_touch_release(),
                    ));
                }
            }
            let terminal = match self.arm9.wait_timed_override("qEmucap,touchstatus", frames) {
                Ok(terminal) => terminal,
                Err(err) => {
                    return Err(cleanup_timed_override_error(
                        err,
                        self.arm9.send_touch_release(),
                    ))
                }
            };
            return match terminal {
                TimedOverrideTerminal::Completed => Ok(json!({
                    "status": "completed",
                    "x": x,
                    "y": y,
                    "frames": frames,
                    "frames_elapsed": frames,
                    "cpu": "arm9",
                    "state": "running",
                    "override_engaged": false,
                })),
                TimedOverrideTerminal::Interrupted { frames_elapsed } => {
                    self.arm9.send_touch_release()?;
                    Ok(json!({
                        "status": "interrupted",
                        "reason": "breakpoint",
                        "x": x,
                        "y": y,
                        "frames": frames,
                        "frames_elapsed": frames_elapsed,
                        "cpu": "arm9",
                        "state": "frozen",
                        "override_engaged": false,
                    }))
                }
            };
        }
        self.arm9.send_touch(x as u16, y as u16, frames)?;
        Ok(json!({
            "x": x,
            "y": y,
            "frames": frames,
            "cpu": "arm9",
            "override_engaged": true,
        }))
    }

    /// Write a native DeSmuME savestate to `path`. Savestates are global (both cores + PPU/SPU),
    /// so the command rides the ARM9 connection. The emulator should be frozen when this runs.
    pub(super) fn save_state(&mut self, params: &Value) -> NdsResult<Value> {
        let path = required_str(params, "path")?.to_string();
        self.arm9.savestate(&path, false)?;
        Ok(json!({ "path": path, "status": "completed" }))
    }

    /// Restore a native DeSmuME savestate from `path`.
    pub(super) fn load_state(&mut self, params: &Value) -> NdsResult<Value> {
        let path = required_str(params, "path")?.to_string();
        self.arm9.savestate(&path, true)?;
        Ok(json!({ "path": path, "status": "completed" }))
    }

    /// Power-cycle the NDS via the DeSmuME fork hook (`QEmucap,reset` → NDS_Reset). Both cores
    /// return to the HLE direct-boot entry and stay halted; issued on the ARM9 connection
    /// (reset is global). Stub-side breakpoints survive the reset, so `bps` is left intact.
    pub(super) fn reset(&mut self, _params: &Value) -> NdsResult<Value> {
        // reset의 계약은 state:"frozen" — 코어를 halt 상태로 남긴다. 하지만 send_cmd는 with_frozen을 거쳐
        // running 코어를 잠깐 pause했다가 reset 후 resume해버린다. 그러면 frozen=true는 거짓이 되고(실제 running)
        // 다음 명령이 with_frozen을 건너뛰어 running 스텁에 나가 desync된다. 그래서 send_cmd 전에 두 코어를
        // 명시적으로 pause해 frozen에서 보내고, reset 후에도 halt가 유지되게 한다.
        self.arm9.pause()?;
        if let Some(a7) = self.arm7.as_mut() {
            a7.pause()?;
        }
        self.arm9.drain_stops()?;
        let resp = self.arm9.send_cmd("QEmucap,reset")?;
        if resp != "OK" {
            return Err(NdsBridgeError::Emulator(format!("reset failed: {resp}")));
        }
        self.arm9.frozen = true;
        if let Some(a7) = self.arm7.as_mut() {
            a7.frozen = true;
        }
        Ok(json!({ "status": "completed", "state": "frozen" }))
    }
}
